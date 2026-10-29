use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use serde::de::{DeserializeSeed, Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::export::ExportResult;

#[derive(Deserialize)]
struct CatalogEntry {
    #[serde(rename = "ResourceType")]
    resource_type: u32,
    #[serde(rename = "StrLabelCrc")]
    label: Option<String>,
    #[serde(rename = "StrTypeCrc")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct DependencyEntry {
    #[serde(rename = "ResourceType")]
    resource_type: u32,
    #[serde(rename = "StrLabelCrc")]
    label: Option<String>,
    #[serde(rename = "StrDepCrcs", default)]
    dependencies: Option<Vec<String>>,
}

pub fn collect_dependencies(
    input: &Path,
    catalog: &Path,
    budget: u64,
) -> ExportResult<Vec<PathBuf>> {
    let root = input
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("根原包名称不是有效 UTF-8")?;
    let mut parser = serde_json::Deserializer::from_reader(BufReader::new(File::open(catalog)?));
    let index = DependencyIndex { budget }.deserialize(&mut parser)?;
    parser.end()?;
    let mut selected = HashSet::new();
    let mut queue = vec![root.to_owned()];
    while let Some(label) = queue.pop() {
        if !selected.insert(label.clone()) {
            continue;
        }
        let dependencies = index
            .get(&label)
            .ok_or_else(|| format!("catalog 未声明 AssetBundle 依赖：{label}"))?;
        queue.extend(dependencies.iter().cloned());
    }
    selected.remove(root);
    let mut dependencies = selected.into_iter().collect::<Vec<_>>();
    dependencies.sort();
    let directory = input.parent().ok_or("根原包没有父目录")?;
    dependencies
        .into_iter()
        .map(|label| {
            let path = directory.join(format!("{label}.assetbundle"));
            if !path.is_file() {
                return Err(format!("明确依赖原包缺失：{}", path.display()).into());
            }
            Ok(path)
        })
        .collect()
}

struct DependencyIndex {
    budget: u64,
}

impl<'de> DeserializeSeed<'de> for DependencyIndex {
    type Value = HashMap<String, Vec<String>>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for DependencyIndex {
    type Value = HashMap<String, Vec<String>>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("带明确依赖的 catalog 资源数组")
    }

    fn visit_seq<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut index = HashMap::new();
        let mut estimated = 0_u64;
        while let Some(entry) = entries.next_element::<DependencyEntry>()? {
            if entry.resource_type != 1 {
                continue;
            }
            let label = entry
                .label
                .ok_or_else(|| A::Error::custom("AssetBundle 条目缺少逻辑名"))?;
            validate_label(&label).map_err(A::Error::custom)?;
            let dependencies = entry.dependencies.unwrap_or_default();
            for dependency in &dependencies {
                validate_label(dependency).map_err(A::Error::custom)?;
            }
            estimated = estimated
                .checked_add(192 + label.len() as u64)
                .and_then(|value| {
                    value.checked_add(
                        dependencies
                            .iter()
                            .map(|dependency| 48 + dependency.len() as u64)
                            .sum::<u64>(),
                    )
                })
                .ok_or_else(|| A::Error::custom("依赖索引预算溢出"))?;
            if estimated > self.budget {
                return Err(A::Error::custom("catalog 依赖索引超过软预算"));
            }
            if index.insert(label, dependencies).is_some() {
                return Err(A::Error::custom("catalog 存在重复 AssetBundle 逻辑名"));
            }
        }
        Ok(index)
    }
}

pub fn collect_inputs(
    inputs: &[PathBuf],
    catalog: Option<&Path>,
    labels: &[String],
    types: &[String],
) -> ExportResult<Vec<PathBuf>> {
    if inputs.is_empty() {
        return Err("请指定 --input 文件或目录".into());
    }
    if let Some(catalog) = catalog {
        if inputs.len() != 1 || !inputs[0].is_dir() {
            return Err("使用 catalog 时，--input 必须是唯一的 plain 目录".into());
        }
        let root = inputs[0].canonicalize()?;
        let mut parser = serde_json::Deserializer::from_reader(BufReader::with_capacity(
            64 * 1024,
            File::open(catalog)?,
        ));
        let paths = CatalogSelection {
            root: &root,
            labels,
            types,
        }
        .deserialize(&mut parser)?;
        parser.end()?;
        return sorted_unique(paths);
    }
    if !labels.is_empty() || !types.is_empty() {
        return Err("--label 和 --type 需要同时提供 --catalog".into());
    }
    let mut paths = Vec::new();
    for input in inputs {
        if input.is_dir() {
            for entry in fs::read_dir(input)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "assetbundle")
                {
                    paths.push(entry.path());
                }
            }
            continue;
        }
        if !input.is_file() {
            return Err(format!("输入文件不存在：{}", input.display()).into());
        }
        if input
            .extension()
            .is_none_or(|extension| extension != "assetbundle")
        {
            return Err(format!("输入不是 .assetbundle 文件：{}", input.display()).into());
        }
        paths.push(input.to_owned());
    }
    sorted_unique(paths)
}

struct CatalogSelection<'a> {
    root: &'a Path,
    labels: &'a [String],
    types: &'a [String],
}

impl<'de> DeserializeSeed<'de> for CatalogSelection<'_> {
    type Value = Vec<PathBuf>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for CatalogSelection<'_> {
    type Value = Vec<PathBuf>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("游戏 catalog 的资源数组")
    }

    fn visit_seq<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut paths = Vec::new();
        let mut found = HashSet::new();
        while let Some(entry) = entries.next_element::<CatalogEntry>()? {
            if entry.resource_type != 1 {
                continue;
            }
            let label = entry
                .label
                .ok_or_else(|| A::Error::custom("AssetBundle 条目缺少 StrLabelCrc"))?;
            if !self.labels.is_empty() && !self.labels.contains(&label) {
                continue;
            }
            let kind = entry
                .kind
                .ok_or_else(|| A::Error::custom(format!("资源 {label} 缺少 StrTypeCrc")))?;
            if !self.types.is_empty() && !self.types.contains(&kind) {
                continue;
            }
            validate_label(&label).map_err(A::Error::custom)?;
            let path = self.root.join(format!("{label}.assetbundle"));
            if !path.is_file() {
                return Err(A::Error::custom(format!(
                    "catalog 对应原包不存在：{}",
                    path.display()
                )));
            }
            found.insert(label);
            paths.push(path);
        }
        let missing = self
            .labels
            .iter()
            .filter(|label| !found.contains(*label))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(A::Error::custom(format!(
                "下列标签没有匹配的 AssetBundle，或被 --type 排除：{}",
                missing
                    .iter()
                    .map(|label| label.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            )));
        }
        Ok(paths)
    }
}

fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty()
        || label.contains(['/', '\\', ':', '\0'])
        || Path::new(label).components().count() != 1
        || !matches!(
            Path::new(label).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(format!(
            "catalog 逻辑名必须是 plain 目录内的单一文件名：{label:?}"
        ));
    }
    Ok(())
}

fn sorted_unique(paths: Vec<PathBuf>) -> ExportResult<Vec<PathBuf>> {
    let mut paths = paths
        .iter()
        .map(|path| path.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::de::DeserializeSeed;

    use super::{CatalogSelection, collect_dependencies, collect_inputs, validate_label};

    static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn with_catalog(data: &[u8], check: impl FnOnce(&Path, &Path)) {
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let identifier = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = temporary.join(format!(
            "rurino-catalog-{}-{identifier}-{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let files = ["one.assetbundle", "two.assetbundle", "catalog.json"];
        fs::write(root.join(files[0]), b"one").unwrap();
        fs::write(root.join(files[1]), b"two").unwrap();
        fs::write(root.join(files[2]), data).unwrap();
        check(&root, &root.join(files[2]));
        assert_eq!(
            root.canonicalize().unwrap().parent(),
            Some(temporary.as_path())
        );
        for file in files {
            fs::remove_file(root.join(file)).unwrap();
        }
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn filters_exact_label_and_type_intersection() {
        let data = br#"[{"ResourceType":1,"StrLabelCrc":"one","StrTypeCrc":"png"},{"ResourceType":1,"StrLabelCrc":"two","StrTypeCrc":"prefab"}]"#;
        with_catalog(data, |root, catalog| {
            let selected = collect_inputs(
                &[root.to_owned()],
                Some(catalog),
                &["one".into()],
                &["png".into()],
            )
            .unwrap();
            assert_eq!(
                selected,
                [root.join("one.assetbundle").canonicalize().unwrap()]
            );
            assert!(
                collect_inputs(
                    &[root.to_owned()],
                    Some(catalog),
                    &["one".into()],
                    &["prefab".into()]
                )
                .is_err()
            );
        });
    }

    #[test]
    fn empty_labels_keep_all_assetbundle_entries() {
        let data = br#"[{"ResourceType":1,"StrLabelCrc":"one","StrTypeCrc":"png"},{"ResourceType":128,"StrLabelCrc":"audio","StrTypeCrc":"acb"}]"#;
        with_catalog(data, |root, catalog| {
            let selected = collect_inputs(&[root.to_owned()], Some(catalog), &[], &[]).unwrap();
            assert_eq!(
                selected,
                [root.join("one.assetbundle").canonicalize().unwrap()]
            );
        });
    }

    #[test]
    fn directory_inputs_are_sorted_and_deduplicated() {
        with_catalog(b"[]", |root, _| {
            let paths = collect_inputs(
                &[root.to_owned(), root.join("one.assetbundle")],
                None,
                &[],
                &[],
            )
            .unwrap();
            assert_eq!(paths.len(), 2);
            assert!(paths[0] < paths[1]);
            assert!(paths.iter().all(|path| path.is_absolute()));
        });
    }

    #[test]
    fn missing_selected_bundle_is_not_silently_skipped() {
        with_catalog(
            br#"[{"ResourceType":1,"StrLabelCrc":"missing","StrTypeCrc":"png"}]"#,
            |root, catalog| {
                assert!(collect_inputs(&[root.to_owned()], Some(catalog), &[], &[]).is_err());
            },
        );
    }

    #[test]
    fn model_dependency_cycles_are_deduplicated_without_loading_other_entries() {
        with_catalog(
            br#"[{"ResourceType":1,"StrLabelCrc":"one","StrDepCrcs":["two","two"]},{"ResourceType":1,"StrLabelCrc":"two","StrDepCrcs":["one"]},{"ResourceType":1,"StrLabelCrc":"unused","StrDepCrcs":[]}]"#,
            |root, catalog| {
                assert_eq!(
                    collect_dependencies(&root.join("one.assetbundle"), catalog, 1024 * 1024)
                        .unwrap(),
                    [root.join("two.assetbundle")]
                );
            },
        );
    }

    #[test]
    fn model_dependency_missing_target_and_metadata_budget_are_errors() {
        with_catalog(
            br#"[{"ResourceType":1,"StrLabelCrc":"one","StrDepCrcs":["missing"]}]"#,
            |root, catalog| {
                assert!(
                    collect_dependencies(&root.join("one.assetbundle"), catalog, 1024 * 1024)
                        .is_err()
                );
                assert!(collect_dependencies(&root.join("one.assetbundle"), catalog, 1).is_err());
            },
        );
    }

    #[test]
    fn label_rejects_paths_and_windows_streams() {
        for label in [
            "", ".", "..", "../one", "dir/one", "dir\\one", "C:one", "one\0",
        ] {
            assert!(validate_label(label).is_err(), "{label:?}");
        }
        assert!(validate_label("__reflection probe-0.exr").is_ok());
    }

    #[test]
    fn catalog_requires_top_level_array() {
        let mut parser = serde_json::Deserializer::from_slice(br#"{"ResourceType":1}"#);
        assert!(
            CatalogSelection {
                root: Path::new("."),
                labels: &[],
                types: &[]
            }
            .deserialize(&mut parser)
            .is_err()
        );
    }
}
