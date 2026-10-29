mod catalog;
mod encoding;
mod export;
mod loading;
mod model;
mod model_glb;
mod model_source;

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use unity_rs_core::image_export::PngCompression;

use export::{
    ExportFormat, ExportRequest, ObjectExport, OutputContent, OutputDescriptor, OutputSink,
    WrittenOutput, export_object,
};
use loading::load_bundle;

const MIB: u64 = 1024 * 1024;

#[derive(Parser)]
#[command(
    version,
    about = "按需读取、有界输出与无损导出的 Unity AssetBundle 工具"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Model {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long = "dependency", conflicts_with = "catalog")]
        dependencies: Vec<PathBuf>,
        #[arg(long, allow_hyphen_values = true)]
        root_path_id: Option<i64>,
        #[arg(long, default_value_t = 0)]
        root_file_index: usize,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "256MiB", value_parser = parse_bytes)]
        memory_budget: u64,
        #[arg(long, default_value = "64MiB", value_parser = parse_bytes)]
        output_budget: u64,
    },
    Inspect {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, default_value = "256MiB", value_parser = parse_bytes)]
        memory_budget: u64,
    },
    Plan {
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        output: PathBuf,
    },
    Extract {
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        plan: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "256MiB", value_parser = parse_bytes)]
        memory_budget: u64,
        #[arg(long, default_value = "512MiB", value_parser = parse_bytes)]
        output_budget: u64,
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        max_objects: Option<usize>,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=16))]
        jobs: u32,
    },
    Verify {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, value_enum, default_value = "decoded")]
        level: VerifyLevel,
        #[arg(long, default_value = "256MiB", value_parser = parse_bytes)]
        memory_budget: u64,
    },
}

#[derive(Args, Clone)]
struct SourceArgs {
    #[arg(long)]
    input: Vec<PathBuf>,
    #[arg(long)]
    catalog: Option<PathBuf>,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long = "type")]
    types: Vec<String>,
}

#[derive(Args, Clone, Serialize, Deserialize)]
struct Selection {
    #[arg(long = "class")]
    classes: Vec<i32>,
    #[arg(long = "path-id", allow_hyphen_values = true)]
    path_ids: Vec<i64>,
    #[arg(long, value_enum, default_value = "native")]
    format: Format,
    #[arg(long, value_enum, default_value = "0")]
    mips: Mips,
    #[arg(long, value_enum, default_value = "fast")]
    profile: Profile,
    #[arg(long, default_value_t = 0)]
    alpha8_rgb: u8,
    #[arg(long, value_enum, default_value = "none")]
    #[serde(default)]
    optimize: Optimize,
    #[arg(long, default_value_t = 5)]
    #[serde(default = "default_optimize_seconds")]
    optimize_seconds: u64,
}

#[derive(Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Format {
    Native,
    Png,
    Webp,
    Auto,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Optimize {
    #[default]
    None,
    Balanced,
    Small,
}

fn default_optimize_seconds() -> u64 {
    5
}

#[derive(Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
enum Mips {
    #[value(name = "0")]
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "all")]
    All,
}

#[derive(Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Profile {
    Fast,
    Default,
    Best,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum VerifyLevel {
    Files,
    Decoded,
}

#[derive(Serialize, Deserialize)]
struct PlanItem {
    schema: u32,
    input: PathBuf,
    input_sha256: String,
    selection: Selection,
}

#[derive(Serialize)]
struct ObjectRecord {
    schema: u32,
    input: String,
    input_sha256: String,
    entry: String,
    path_id: String,
    class_id: i32,
    policy_key: String,
    #[serde(flatten)]
    outcome: ObjectExport,
}

#[derive(Deserialize)]
struct StoredRecord {
    input_sha256: String,
    entry: String,
    path_id: String,
    policy_key: String,
    status: String,
    outputs: Vec<StoredOutput>,
}

#[derive(Deserialize)]
struct StoredOutput {
    descriptor: StoredDescriptor,
    output: StoredFile,
}

#[derive(Deserialize)]
struct StoredDescriptor {
    rgba_sha256: Option<String>,
    source_payload_sha256: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Deserialize)]
struct StoredFile {
    path: String,
    bytes: u64,
    sha256: String,
}

struct FileSink<'a> {
    root: &'a Path,
    prefix: PathBuf,
    output_budget: u64,
    used: &'a AtomicU64,
    published: Vec<(PathBuf, u64)>,
}

struct QuotaWriter<'a> {
    output: BufWriter<File>,
    hash: Sha256,
    bytes: u64,
    used: &'a AtomicU64,
    limit: u64,
}

impl Write for QuotaWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let length = data.len() as u64;
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(length).filter(|next| *next <= self.limit)
            })
            .map_err(|_| std::io::Error::other("输出配额不足"))?;
        if let Err(error) = self.output.write_all(data) {
            self.used.fetch_sub(length, Ordering::AcqRel);
            return Err(error);
        }
        self.hash.update(data);
        self.bytes += length;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

impl OutputSink for FileSink<'_> {
    fn write(
        &mut self,
        descriptor: &OutputDescriptor,
        content: OutputContent<'_>,
    ) -> export::ExportResult<WrittenOutput> {
        Ok(self
            .write_content(descriptor, |writer| content.write_to(writer))?
            .0)
    }
}

impl FileSink<'_> {
    fn write_content<T>(
        &mut self,
        descriptor: &OutputDescriptor,
        operation: impl FnOnce(&mut QuotaWriter<'_>) -> export::ExportResult<T>,
    ) -> export::ExportResult<(WrittenOutput, T)> {
        let leaf = Path::new(&descriptor.relative_name);
        if !safe_relative(&descriptor.relative_name) {
            return Err("输出名称必须是目录内的相对路径".into());
        }
        let relative = self.prefix.join(leaf);
        let target = self.root.join(&relative);
        fs::create_dir_all(target.parent().ok_or("输出目录无父路径")?)?;
        let temporary = target.with_extension("part");
        if temporary.exists() {
            let previous_bytes = temporary.metadata()?.len();
            fs::remove_file(&temporary)?;
            self.used.fetch_sub(previous_bytes, Ordering::AcqRel);
        }
        let mut writer = QuotaWriter {
            output: BufWriter::with_capacity(64 * 1024, File::create(&temporary)?),
            hash: Sha256::new(),
            bytes: 0,
            used: self.used,
            limit: self.output_budget,
        };
        let result = operation(&mut writer).and_then(|value| {
            writer.flush()?;
            Ok(value)
        });
        let bytes = writer.bytes;
        let sha256 = format!("{:x}", writer.hash.clone().finalize());
        drop(writer);
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.used.fetch_sub(bytes, Ordering::AcqRel);
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        if target.exists() {
            let previous_bytes = target.metadata()?.len();
            fs::remove_file(&target)?;
            self.used.fetch_sub(previous_bytes, Ordering::AcqRel);
        }
        fs::rename(temporary, &target)?;
        self.published.push((target, bytes));
        Ok((
            WrittenOutput {
                path: relative.to_string_lossy().replace('\\', "/"),
                bytes,
                sha256,
            },
            value,
        ))
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("错误：{error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> export::ExportResult<()> {
    match cli.command {
        Command::Model {
            input,
            catalog,
            dependencies,
            root_path_id,
            root_file_index,
            output,
            memory_budget,
            output_budget,
        } => model::extract_model(model::ModelOptions {
            input,
            catalog,
            dependencies,
            root_path_id,
            root_file_index,
            output,
            memory_budget,
            output_budget,
        }),
        Command::Inspect {
            source,
            memory_budget,
        } => inspect(&source, memory_budget),
        Command::Plan {
            source,
            selection,
            output,
        } => {
            let mut writer = BufWriter::new(File::create(output)?);
            for input in source_paths(&source)? {
                write_json_line(
                    &mut writer,
                    &PlanItem {
                        schema: 1,
                        input_sha256: hash_file(&input)?,
                        input: fs::canonicalize(input)?,
                        selection: selection.clone(),
                    },
                )?;
            }
            writer.flush()?;
            Ok(())
        }
        Command::Extract {
            source,
            selection,
            plan,
            output,
            memory_budget,
            output_budget,
            resume,
            max_objects,
            jobs,
        } => extract(
            &source,
            &selection,
            plan.as_deref(),
            &output,
            memory_budget,
            output_budget,
            resume,
            max_objects,
            jobs,
        ),
        Command::Verify {
            manifest,
            level,
            memory_budget,
        } => verify(&manifest, level, memory_budget),
    }
}

fn inspect(source: &SourceArgs, memory_budget: u64) -> export::ExportResult<()> {
    let inputs = source_paths(source)?;
    if inputs.is_empty() {
        return Err("请指定 --input 文件或目录".into());
    }
    let started = Instant::now();
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    for input in &inputs {
        let loaded = load_bundle(input, memory_budget)?;
        write_json_line(
            &mut writer,
            &json!({
                "event": "bundle",
                "input": input,
                "stats": loaded.stats,
                "io": loaded.io.snapshot(),
            }),
        )?;
    }
    write_json_line(
        &mut writer,
        &json!({
            "event": "summary",
            "bundles": inputs.len(),
            "elapsed_seconds": started.elapsed().as_secs_f64(),
        }),
    )?;
    writer.flush()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn extract(
    source: &SourceArgs,
    selection: &Selection,
    plan: Option<&Path>,
    output: &Path,
    memory_budget: u64,
    output_budget: u64,
    resume: bool,
    max_objects: Option<usize>,
    jobs: u32,
) -> export::ExportResult<()> {
    let tasks = if let Some(plan) = plan {
        if !source.input.is_empty() || source.catalog.is_some() {
            return Err("--plan与--input/--catalog不能同时使用".into());
        }
        read_plan(plan, memory_budget / 8)?
    } else {
        source_paths(source)?
            .into_iter()
            .map(|input| {
                Ok(PlanItem {
                    schema: 1,
                    input_sha256: String::new(),
                    input,
                    selection: selection.clone(),
                })
            })
            .collect::<export::ExportResult<Vec<_>>>()?
    };
    if tasks.is_empty() {
        return Err("请指定 --input 文件或目录".into());
    }
    fs::create_dir_all(output)?;
    let manifest = output.join("manifest.jsonl");
    if manifest.exists() && !resume {
        return Err("输出清单已存在，请使用 --resume 或选择新目录".into());
    }
    let previous = if resume {
        read_previous(&manifest, memory_budget / 8, true)?
    } else {
        HashMap::new()
    };
    let journal = Arc::new(Mutex::new(BufWriter::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&manifest)?,
    )));
    let used = AtomicU64::new(existing_output_bytes(output)?);
    if used.load(Ordering::Relaxed) > output_budget {
        return Err("现有输出已超过配额".into());
    }
    let started = Instant::now();
    let counters = Counters::default();
    let workers = (jobs as usize).min(tasks.len());
    let worker_budget = memory_budget / workers as u64;
    if worker_budget < 32 * MIB {
        return Err("每个任务至少需要32MiB软预算，请降低--jobs".into());
    }
    let queue = Mutex::new(tasks.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let task = queue.lock().expect("任务锁异常").next();
                    let Some(task) = task else { break };
                    if let Err(error) = extract_bundle(
                        &task,
                        output,
                        worker_budget,
                        output_budget,
                        &used,
                        &previous,
                        &journal,
                        &counters,
                        max_objects,
                    ) {
                        counters.failed.fetch_add(1, Ordering::Relaxed);
                        eprintln!("原包失败 {}：{error}", task.input.display());
                    }
                }
            });
        }
    });
    journal.lock().map_err(|_| "清单锁异常")?.flush()?;
    println!(
        "{}",
        json!({
            "event": "summary", "selected": counters.selected.load(Ordering::Relaxed),
            "succeeded": counters.succeeded.load(Ordering::Relaxed),
            "failed": counters.failed.load(Ordering::Relaxed),
            "resumed": counters.resumed.load(Ordering::Relaxed),
            "statuses": *counters.statuses.lock().map_err(|_| "状态锁异常")?,
            "output_bytes": used.load(Ordering::Relaxed), "elapsed_seconds": started.elapsed().as_secs_f64(),
            "memory_budget_bytes": memory_budget, "workers": workers,
            "per_worker_budget_bytes": worker_budget,
        })
    );
    if counters.failed.load(Ordering::Relaxed) > 0 {
        return Err("部分对象导出失败，详见manifest.jsonl".into());
    }
    Ok(())
}

#[derive(Default)]
struct Counters {
    selected: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    resumed: AtomicU64,
    statuses: Mutex<BTreeMap<String, u64>>,
}

#[allow(clippy::too_many_arguments)]
fn extract_bundle(
    task: &PlanItem,
    output: &Path,
    memory_budget: u64,
    output_budget: u64,
    used: &AtomicU64,
    previous: &HashMap<String, StoredRecord>,
    journal: &Mutex<BufWriter<File>>,
    counters: &Counters,
    max_objects: Option<usize>,
) -> export::ExportResult<()> {
    if task.schema != 1 {
        return Err("计划schema不支持".into());
    }
    let input = &task.input;
    let selection = &task.selection;
    let loaded = load_bundle(input, memory_budget)?;
    let before_hash = loaded.io.snapshot();
    let input_sha256 = export::hash_region(&loaded.source)?;
    let after_hash = loaded.io.snapshot();
    if !task.input_sha256.is_empty() && input_sha256 != task.input_sha256 {
        return Err("原包SHA-256与计划不符".into());
    }
    let decoder_budget = memory_budget.saturating_sub(loaded.stats.admission_estimate_bytes);
    if decoder_budget < 16 * MIB {
        return Err(format!("加载后解码预算不足：{}", input.display()).into());
    }
    let request = ExportRequest {
        format: match selection.format {
            Format::Native => ExportFormat::Native,
            Format::Png => ExportFormat::Png,
            Format::Webp => ExportFormat::Webp,
            Format::Auto => ExportFormat::Auto,
        },
        all_mips: selection.mips == Mips::All,
        decoder_budget_bytes: decoder_budget,
        png_compression: match selection.profile {
            Profile::Fast => PngCompression::Fast,
            Profile::Default => PngCompression::Default,
            Profile::Best => PngCompression::Best,
        },
        alpha8_rgb: selection.alpha8_rgb,
        optimize: match selection.optimize {
            Optimize::None => encoding::OptimizationEffort::None,
            Optimize::Balanced => encoding::OptimizationEffort::Balanced,
            Optimize::Small => encoding::OptimizationEffort::Small,
        },
        optimize_seconds: selection.optimize_seconds,
    };
    let policy_key = hash_bytes(&serde_json::to_vec(&json!({
        "tool": env!("CARGO_PKG_VERSION"), "core": "0.6.1", "export_schema": 1,
            "export_implementation": hash_bytes(include_bytes!("export.rs")),
            "encoding_implementation": hash_bytes(include_bytes!("encoding.rs")),
        "selection": selection,
    }))?);
    let mut processed = 0usize;
    for (file_index, file) in loaded.collection.serialized_files().iter().enumerate() {
        let entry = file.path.rsplit("::").next().unwrap_or(&file.path);
        for (object_index, object) in file.file.objects.iter().enumerate() {
            if !selected_object(object.class_id, object.path_id, selection) {
                continue;
            }
            if max_objects.is_some_and(|maximum| processed >= maximum) {
                break;
            }
            processed += 1;
            counters.selected.fetch_add(1, Ordering::Relaxed);
            let key = record_key(
                &input_sha256,
                entry,
                &object.path_id.to_string(),
                &policy_key,
            );
            if previous
                .get(&key)
                .is_some_and(|row| valid_previous(output, row))
            {
                counters.succeeded.fetch_add(1, Ordering::Relaxed);
                counters.resumed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let mut sink = FileSink {
                root: output,
                prefix: PathBuf::from(&input_sha256)
                    .join(format!("entry-{file_index}"))
                    .join(object.path_id.to_string())
                    .join(&policy_key),
                used,
                output_budget,
                published: Vec::new(),
            };
            let outcome = match export_object(
                &loaded.collection,
                file_index,
                object_index,
                &request,
                &mut sink,
            ) {
                Ok(value) => {
                    *counters
                        .statuses
                        .lock()
                        .map_err(|_| "状态锁异常")?
                        .entry(value.status.clone())
                        .or_insert(0) += 1;
                    if matches!(value.status.as_str(), "raw_verified" | "decoded_verified") {
                        counters.succeeded.fetch_add(1, Ordering::Relaxed);
                    }
                    value
                }
                Err(error) => {
                    counters.failed.fetch_add(1, Ordering::Relaxed);
                    for (path, bytes) in &sink.published {
                        fs::remove_file(path)?;
                        used.fetch_sub(*bytes, Ordering::AcqRel);
                    }
                    ObjectExport {
                        kind: "unknown".into(),
                        status: "decode_error".into(),
                        name: String::new(),
                        metadata: json!({"error": error.to_string()}),
                        outputs: Vec::new(),
                    }
                }
            };
            let record = ObjectRecord {
                schema: 1,
                input: input.to_string_lossy().into_owned(),
                input_sha256: input_sha256.clone(),
                entry: entry.to_owned(),
                path_id: object.path_id.to_string(),
                class_id: object.class_id,
                policy_key: policy_key.clone(),
                outcome,
            };
            let mut journal = journal.lock().map_err(|_| "清单锁异常")?;
            write_json_line(&mut *journal, &record)?;
            journal.flush()?;
        }
    }
    println!(
        "{}",
        json!({
            "event": "bundle_complete",
            "input": input,
            "selected_objects": processed,
            "io": loaded.io.snapshot(),
            "source_hash_read_bytes": after_hash.source_read_bytes - before_hash.source_read_bytes,
            "source_hash_read_calls": after_hash.source_read_calls - before_hash.source_read_calls,
            "selected_read_bytes": loaded.io.snapshot().source_read_bytes
                - (after_hash.source_read_bytes - before_hash.source_read_bytes),
            "loader": loaded.stats,
        })
    );
    Ok(())
}

fn selected_object(class_id: i32, path_id: i64, selection: &Selection) -> bool {
    (selection.classes.is_empty() || selection.classes.contains(&class_id))
        && (selection.path_ids.is_empty() || selection.path_ids.contains(&path_id))
}

fn read_previous(
    path: &Path,
    maximum: u64,
    repair_tail: bool,
) -> export::ExportResult<HashMap<String, StoredRecord>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    if path.metadata()?.len() > maximum {
        return Err("清单超过元数据预算，请拆分任务".into());
    }
    let mut records = HashMap::new();
    let mut reader = BufReader::new(File::open(path)?);
    let mut complete_bytes = 0u64;
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            if !repair_tail {
                return Err("清单末行不完整，请先使用--resume恢复".into());
            }
            OpenOptions::new()
                .write(true)
                .open(path)?
                .set_len(complete_bytes)?;
            break;
        }
        complete_bytes += line.len() as u64;
        let row: StoredRecord = serde_json::from_slice(&line)?;
        records.insert(
            record_key(&row.input_sha256, &row.entry, &row.path_id, &row.policy_key),
            row,
        );
    }
    Ok(records)
}

fn valid_previous(root: &Path, row: &StoredRecord) -> bool {
    row.status != "decode_error"
        && row.outputs.iter().all(|entry| {
            safe_relative(&entry.output.path)
                && fs::metadata(root.join(&entry.output.path))
                    .is_ok_and(|metadata| metadata.len() == entry.output.bytes)
                && hash_file(&root.join(&entry.output.path))
                    .is_ok_and(|hash| hash == entry.output.sha256)
        })
}

fn verify(path: &Path, level: VerifyLevel, memory_budget: u64) -> export::ExportResult<()> {
    let root = path.parent().ok_or("清单没有父目录")?;
    let mut checked = 0u64;
    let mut decoded = 0u64;
    let mut statuses = BTreeMap::new();
    for row in read_previous(path, memory_budget / 8, false)?.into_values() {
        *statuses.entry(row.status.clone()).or_insert(0u64) += 1;
        if row.status == "decode_error" {
            return Err(format!("清单含解码失败对象：{}", row.path_id).into());
        }
        for entry in row.outputs {
            if !safe_relative(&entry.output.path) {
                return Err("清单输出路径超出目录".into());
            }
            let file = root.join(&entry.output.path);
            if fs::metadata(&file)?.len() != entry.output.bytes
                || hash_file(&file)? != entry.output.sha256
            {
                return Err(format!("输出文件校验不符：{}", file.display()).into());
            }
            if level == VerifyLevel::Decoded {
                if file.extension().is_some_and(|value| value == "glb") {
                    return Err(
                        "模型数值核验需要独立源数据与回读工具；文件哈希核验请使用 --level files"
                            .into(),
                    );
                }
                if let Some(expected) = entry.descriptor.rgba_sha256 {
                    if entry.output.bytes > memory_budget / 4 {
                        return Err("图片文件超过验证读取预算".into());
                    }
                    let format = match file.extension().and_then(|value| value.to_str()) {
                        Some("png") => encoding::EncodedFormat::Png,
                        Some("webp") => encoding::EncodedFormat::Webp,
                        _ => return Err("像素验证不支持该图片扩展名".into()),
                    };
                    let image =
                        encoding::decoded_image_hash(&fs::read(&file)?, format, memory_budget)?;
                    if Some(image.width) != entry.descriptor.width
                        || Some(image.height) != entry.descriptor.height
                    {
                        return Err("图片尺寸与清单不符".into());
                    }
                    if image.rgba_sha256 != expected {
                        return Err("图片像素SHA-256不符".into());
                    }
                    decoded += 1;
                } else if entry
                    .descriptor
                    .source_payload_sha256
                    .as_ref()
                    .is_some_and(|expected| *expected != entry.output.sha256)
                {
                    return Err("原生载荷SHA-256不符".into());
                }
            }
            checked += 1;
        }
    }
    println!(
        "{}",
        json!({"event": "verified", "files": checked, "decoded_images": decoded, "statuses": statuses})
    );
    Ok(())
}

fn source_paths(source: &SourceArgs) -> export::ExportResult<Vec<PathBuf>> {
    catalog::collect_inputs(
        &source.input,
        source.catalog.as_deref(),
        &source.labels,
        &source.types,
    )
}

fn read_plan(path: &Path, maximum: u64) -> export::ExportResult<Vec<PlanItem>> {
    if path.metadata()?.len() > maximum {
        return Err("计划超过元数据预算".into());
    }
    BufReader::new(File::open(path)?)
        .lines()
        .map(|line| {
            let task: PlanItem = serde_json::from_str(&line?)?;
            if task.schema != 1
                || task.input_sha256.len() != 64
                || !task
                    .input_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("计划schema或来源SHA-256无效".into());
            }
            Ok(task)
        })
        .collect()
}

fn existing_output_bytes(root: &Path) -> export::ExportResult<u64> {
    let mut queue = vec![root.to_path_buf()];
    let mut size = 0u64;
    while let Some(directory) = queue.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                queue.push(entry.path());
                continue;
            }
            if entry.file_type()?.is_symlink() {
                return Err("输出目录包含链接，请使用独立目录".into());
            }
            if entry.file_name() != "manifest.jsonl" {
                size = size
                    .checked_add(entry.metadata()?.len())
                    .ok_or("输出大小溢出")?;
            }
        }
    }
    Ok(size)
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn write_json_line(writer: &mut dyn Write, value: &impl Serialize) -> export::ExportResult<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(
        b"
",
    )?;
    Ok(())
}

fn record_key(input: &str, entry: &str, path_id: &str, policy: &str) -> String {
    format!("{input}|{entry}|{path_id}|{policy}")
}

fn hash_file(path: &Path) -> export::ExportResult<String> {
    let mut reader = BufReader::with_capacity(64 * 1024, File::open(path)?);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let size = reader.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn hash_bytes(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn parse_bytes(value: &str) -> Result<u64, String> {
    let lower = value.to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = lower.strip_suffix("mib") {
        (number, MIB)
    } else if let Some(number) = lower.strip_suffix("gib") {
        (number, 1024 * MIB)
    } else if let Some(number) = lower.strip_suffix("kib") {
        (number, 1024)
    } else {
        (lower.as_str(), 1)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .filter(|number| *number > 0)
        .ok_or_else(|| "请输入正整数字节数或 KiB/MiB/GiB".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{parse_bytes, safe_relative};

    #[test]
    fn byte_options_require_positive_nonoverflowing_values() {
        assert_eq!(parse_bytes("256MiB").unwrap(), 268435456);
        assert!(parse_bytes("0").is_err());
        assert!(parse_bytes("18446744073709551615GiB").is_err());
    }

    #[test]
    fn output_paths_stay_relative() {
        assert!(safe_relative("bundle/1/payload.bin"));
        assert!(!safe_relative("../bundle/file"));
        assert!(!safe_relative("C:/elsewhere/file"));
    }
}
