# 第三方许可与来源

本清单按 Cargo.lock 和 `cargo metadata --offline --locked --filter-platform x86_64-pc-windows-msvc` 解析的非开发依赖生成，共 91 个第三方 crate；包含目标平台适用的运行与构建依赖。

Rurino 自有代码使用 MIT 许可。第三方依赖各自的许可表达式与版权声明仍适用，不将整个依赖图表述为 MIT。多选许可保留 crate 内全部可用许可文本，AND 许可同时保留其附加文本。

许可文件按依赖源码原始相对路径保存在 `licenses/<crate-version>/`；机器清单 `third-party-licenses.json` 记录每份文本和每个原始 crate 包的 SHA-256。

## MPL-2.0 依赖源码

Symphonia 的四个依赖采用 MPL-2.0，本项目没有修改这些依赖的源码。分发可执行文件时，应同时提供本声明和许可文本，并使接收者能取得相应 MPL 覆盖源码。下列链接是锁定版本的完整 crate 源码，包哈希见机器清单；未来修改这些依赖时需要提供对应修改后的源码。

| 包 | 版本 | 对应源码 |
| --- | --- | --- |
| symphonia | 0.6.1 | [完整 crate 源码](https://crates.io/api/v1/crates/symphonia/0.6.1/download) |
| symphonia-bundle-mp3 | 0.6.1 | [完整 crate 源码](https://crates.io/api/v1/crates/symphonia-bundle-mp3/0.6.1/download) |
| symphonia-core | 0.6.1 | [完整 crate 源码](https://crates.io/api/v1/crates/symphonia-core/0.6.1/download) |
| symphonia-metadata | 0.6.1 | [完整 crate 源码](https://crates.io/api/v1/crates/symphonia-metadata/0.6.1/download) |

MPL-2.0 为这些文件保留其许可，不改变独立自有代码的 MIT 声明。本清单保守列出锁定依赖图，即使某些解码路径未被首版命令直接调用。

## 内嵌和附加许可

- `unity-rs-core` 内嵌修正的 texture2ddecoder 代码及其 MIT 文本，同包的 `THIRD_PARTY_NOTICES.md` 和 `THIRD_PARTY_LICENSES.txt` 已一并保留。
- `FSB_VORBIS_NOTICE.md` 保留 fsbex 0.3.0 生成数据的 MIT 授权与版权声明。
- `unicode-ident` 的 Unicode-3.0、`jpeg-encoder` 的 IJG、`brotli` 的 BSD/MIT，以及 `zstd-sys` 内嵌 Zstandard 的 BSD 声明按源包原文保留。
- 某个发布 crate 若没有独立 LICENSE 文件，仅在上游 core 的合并许可文件存在完全相同的包名和版本段落时复用该文本，并在机器清单标出来源。

## 锁定依赖列表

| 包 | 版本 | 声明许可 | 源码仓库 | 随附许可文本 |
| --- | --- | --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | [来源](https://github.com/oyvindln/adler2) | [LICENSE-0BSD](licenses/adler2-2.0.1/LICENSE-0BSD)、[LICENSE-APACHE](licenses/adler2-2.0.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/adler2-2.0.1/LICENSE-MIT) |
| alloc-no-stdlib | 3.0.0 | BSD-3-Clause | [来源](https://github.com/dropbox/rust-alloc-no-stdlib) | [LICENSE](licenses/alloc-no-stdlib-3.0.0/LICENSE) |
| alloc-stdlib | 0.3.0 | BSD-3-Clause | [来源](https://github.com/dropbox/rust-alloc-no-stdlib) | [UPSTREAM-LICENSE-BUNDLE.txt](licenses/alloc-stdlib-0.3.0/UPSTREAM-LICENSE-BUNDLE.txt) |
| anstream | 1.0.0 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/anstream-1.0.0/LICENSE-APACHE)、[LICENSE-MIT](licenses/anstream-1.0.0/LICENSE-MIT) |
| anstyle | 1.0.14 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/anstyle-1.0.14/LICENSE-APACHE)、[LICENSE-MIT](licenses/anstyle-1.0.14/LICENSE-MIT) |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/anstyle-parse-1.0.0/LICENSE-APACHE)、[LICENSE-MIT](licenses/anstyle-parse-1.0.0/LICENSE-MIT) |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/anstyle-query-1.1.5/LICENSE-APACHE)、[LICENSE-MIT](licenses/anstyle-query-1.1.5/LICENSE-MIT) |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/anstyle-wincon-3.0.11/LICENSE-APACHE)、[LICENSE-MIT](licenses/anstyle-wincon-3.0.11/LICENSE-MIT) |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | [来源](https://github.com/cuviper/autocfg) | [LICENSE-APACHE](licenses/autocfg-1.5.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/autocfg-1.5.1/LICENSE-MIT) |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | [来源](https://github.com/bitflags/bitflags) | [LICENSE-APACHE](licenses/bitflags-2.13.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/bitflags-2.13.2/LICENSE-MIT) |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | [来源](https://github.com/RustCrypto/utils) | [LICENSE-APACHE](licenses/block-buffer-0.10.4/LICENSE-APACHE)、[LICENSE-MIT](licenses/block-buffer-0.10.4/LICENSE-MIT) |
| brotli | 9.1.1 | BSD-3-Clause AND MIT | [来源](https://github.com/dropbox/rust-brotli) | [LICENSE.BSD-3-Clause](licenses/brotli-9.1.1/LICENSE.BSD-3-Clause)、[LICENSE.MIT](licenses/brotli-9.1.1/LICENSE.MIT) |
| brotli-decompressor | 6.2.0 | BSD-3-Clause/MIT | [来源](https://github.com/dropbox/rust-brotli-decompressor) | [LICENSE](licenses/brotli-decompressor-6.2.0/LICENSE) |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | [来源](https://github.com/Lokathor/bytemuck) | [LICENSE-APACHE](licenses/bytemuck-1.25.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/bytemuck-1.25.2/LICENSE-MIT)、[LICENSE-ZLIB](licenses/bytemuck-1.25.2/LICENSE-ZLIB) |
| byteorder | 1.5.0 | Unlicense OR MIT | [来源](https://github.com/BurntSushi/byteorder) | [COPYING](licenses/byteorder-1.5.0/COPYING)、[LICENSE-MIT](licenses/byteorder-1.5.0/LICENSE-MIT)、[UNLICENSE](licenses/byteorder-1.5.0/UNLICENSE) |
| byteorder-lite | 0.1.0 | Unlicense OR MIT | [来源](https://github.com/image-rs/byteorder-lite) | [LICENSE-MIT](licenses/byteorder-lite-0.1.0/LICENSE-MIT)、[UNLICENSE](licenses/byteorder-lite-0.1.0/UNLICENSE) |
| cc | 1.10.0 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/cc-rs) | [LICENSE-APACHE](licenses/cc-1.10.0/LICENSE-APACHE)、[LICENSE-MIT](licenses/cc-1.10.0/LICENSE-MIT) |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/cfg-if) | [LICENSE-APACHE](licenses/cfg-if-1.0.5/LICENSE-APACHE)、[LICENSE-MIT](licenses/cfg-if-1.0.5/LICENSE-MIT) |
| clap | 4.6.7 | MIT OR Apache-2.0 | [来源](https://github.com/clap-rs/clap) | [LICENSE-APACHE](licenses/clap-4.6.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/clap-4.6.7/LICENSE-MIT) |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 | [来源](https://github.com/clap-rs/clap) | [LICENSE-APACHE](licenses/clap_builder-4.6.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/clap_builder-4.6.7/LICENSE-MIT) |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 | [来源](https://github.com/clap-rs/clap) | [LICENSE-APACHE](licenses/clap_derive-4.6.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/clap_derive-4.6.7/LICENSE-MIT) |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 | [来源](https://github.com/clap-rs/clap) | [LICENSE-APACHE](licenses/clap_lex-1.1.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/clap_lex-1.1.1/LICENSE-MIT) |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 | [来源](https://github.com/rust-cli/anstyle.git) | [LICENSE-APACHE](licenses/colorchoice-1.0.5/LICENSE-APACHE)、[LICENSE-MIT](licenses/colorchoice-1.0.5/LICENSE-MIT) |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | [来源](https://github.com/RustCrypto/utils) | [LICENSE-APACHE](licenses/cpufeatures-0.2.17/LICENSE-APACHE)、[LICENSE-MIT](licenses/cpufeatures-0.2.17/LICENSE-MIT) |
| crc32fast | 1.5.3 | MIT OR Apache-2.0 | [来源](https://github.com/srijs/rust-crc32fast) | [LICENSE-APACHE](licenses/crc32fast-1.5.3/LICENSE-APACHE)、[LICENSE-MIT](licenses/crc32fast-1.5.3/LICENSE-MIT) |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | [来源](https://github.com/RustCrypto/traits) | [LICENSE-APACHE](licenses/crypto-common-0.1.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/crypto-common-0.1.7/LICENSE-MIT) |
| digest | 0.10.7 | MIT OR Apache-2.0 | [来源](https://github.com/RustCrypto/traits) | [LICENSE-APACHE](licenses/digest-0.10.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/digest-0.10.7/LICENSE-MIT) |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | [来源](https://github.com/indexmap-rs/equivalent) | [LICENSE-APACHE](licenses/equivalent-1.0.3/LICENSE-APACHE)、[LICENSE-MIT](licenses/equivalent-1.0.3/LICENSE-MIT) |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | [来源](https://github.com/image-rs/fdeflate) | [LICENSE-APACHE](licenses/fdeflate-0.3.7/LICENSE-APACHE)、[LICENSE-MIT](licenses/fdeflate-0.3.7/LICENSE-MIT) |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/cc-rs) | [LICENSE-APACHE](licenses/find-msvc-tools-0.1.15/LICENSE-APACHE)、[LICENSE-MIT](licenses/find-msvc-tools-0.1.15/LICENSE-MIT) |
| flate2 | 1.1.10 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/flate2-rs) | [LICENSE-APACHE](licenses/flate2-1.1.10/LICENSE-APACHE)、[LICENSE-MIT](licenses/flate2-1.1.10/LICENSE-MIT) |
| generic-array | 0.14.7 | MIT | [来源](https://github.com/fizyk20/generic-array.git) | [LICENSE](licenses/generic-array-0.14.7/LICENSE) |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | [来源](https://github.com/rust-random/getrandom) | [LICENSE-APACHE](licenses/getrandom-0.4.3/LICENSE-APACHE)、[LICENSE-MIT](licenses/getrandom-0.4.3/LICENSE-MIT) |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/hashbrown) | [LICENSE-APACHE](licenses/hashbrown-0.17.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/hashbrown-0.17.1/LICENSE-MIT) |
| heck | 0.5.0 | MIT OR Apache-2.0 | [来源](https://github.com/withoutboats/heck) | [LICENSE-APACHE](licenses/heck-0.5.0/LICENSE-APACHE)、[LICENSE-MIT](licenses/heck-0.5.0/LICENSE-MIT) |
| image-webp | 0.2.4 | MIT OR Apache-2.0 | [来源](https://github.com/image-rs/image-webp) | [LICENSE-APACHE](licenses/image-webp-0.2.4/LICENSE-APACHE)、[LICENSE-MIT](licenses/image-webp-0.2.4/LICENSE-MIT) |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | [来源](https://github.com/indexmap-rs/indexmap) | [LICENSE-APACHE](licenses/indexmap-2.14.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/indexmap-2.14.2/LICENSE-MIT) |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 | [来源](https://github.com/polyfill-rs/is_terminal_polyfill) | [LICENSE-APACHE](licenses/is_terminal_polyfill-1.70.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/is_terminal_polyfill-1.70.2/LICENSE-MIT) |
| itoa | 1.0.18 | MIT OR Apache-2.0 | [来源](https://github.com/dtolnay/itoa) | [LICENSE-APACHE](licenses/itoa-1.0.18/LICENSE-APACHE)、[LICENSE-MIT](licenses/itoa-1.0.18/LICENSE-MIT) |
| jobserver | 0.1.35 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/jobserver-rs) | [LICENSE-APACHE](licenses/jobserver-0.1.35/LICENSE-APACHE)、[LICENSE-MIT](licenses/jobserver-0.1.35/LICENSE-MIT) |
| jpeg-encoder | 0.7.1 | (MIT OR Apache-2.0) AND IJG | [来源](https://github.com/vstroebel/jpeg-encoder) | [LICENSE-APACHE](licenses/jpeg-encoder-0.7.1/LICENSE-APACHE)、[LICENSE-IJG](licenses/jpeg-encoder-0.7.1/LICENSE-IJG)、[LICENSE-MIT](licenses/jpeg-encoder-0.7.1/LICENSE-MIT) |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang-nursery/lazy-static.rs) | [LICENSE-APACHE](licenses/lazy_static-1.5.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/lazy_static-1.5.1/LICENSE-MIT) |
| lewton | 0.10.2 | MIT OR Apache-2.0 | [来源](https://github.com/RustAudio/lewton) | [LICENSE](licenses/lewton-0.10.2/LICENSE) |
| log | 0.4.34 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/log) | [LICENSE-APACHE](licenses/log-0.4.34/LICENSE-APACHE)、[LICENSE-MIT](licenses/log-0.4.34/LICENSE-MIT) |
| lz4_flex | 0.14.0 | MIT | [来源](https://github.com/pseitz/lz4_flex) | [LICENSE](licenses/lz4_flex-0.14.0/LICENSE) |
| lzma-rust2 | 0.21.0 | Apache-2.0 | [来源](https://github.com/hasenbanck/lzma-rust2/) | [LICENSE](licenses/lzma-rust2-0.21.0/LICENSE) |
| memchr | 2.8.3 | Unlicense OR MIT | [来源](https://github.com/BurntSushi/memchr) | [COPYING](licenses/memchr-2.8.3/COPYING)、[LICENSE-MIT](licenses/memchr-2.8.3/LICENSE-MIT)、[UNLICENSE](licenses/memchr-2.8.3/UNLICENSE) |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | [来源](https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide) | [LICENSE](licenses/miniz_oxide-0.8.9/LICENSE)、[LICENSE-APACHE.md](licenses/miniz_oxide-0.8.9/LICENSE-APACHE.md)、[LICENSE-MIT.md](licenses/miniz_oxide-0.8.9/LICENSE-MIT.md)、[LICENSE-ZLIB.md](licenses/miniz_oxide-0.8.9/LICENSE-ZLIB.md) |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | [来源](https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide) | [LICENSE](licenses/miniz_oxide-0.9.1/LICENSE)、[LICENSE-APACHE.md](licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md)、[LICENSE-MIT.md](licenses/miniz_oxide-0.9.1/LICENSE-MIT.md)、[LICENSE-ZLIB.md](licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md) |
| num-complex | 0.4.6 | MIT OR Apache-2.0 | [来源](https://github.com/rust-num/num-complex) | [LICENSE-APACHE](licenses/num-complex-0.4.6/LICENSE-APACHE)、[LICENSE-MIT](licenses/num-complex-0.4.6/LICENSE-MIT) |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | [来源](https://github.com/rust-num/num-traits) | [LICENSE-APACHE](licenses/num-traits-0.2.19/LICENSE-APACHE)、[LICENSE-MIT](licenses/num-traits-0.2.19/LICENSE-MIT) |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 | [来源](https://github.com/polyfill-rs/once_cell_polyfill) | [LICENSE-APACHE](licenses/once_cell_polyfill-1.70.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/once_cell_polyfill-1.70.2/LICENSE-MIT) |
| paste | 1.0.15 | MIT OR Apache-2.0 | [来源](https://github.com/dtolnay/paste) | [LICENSE-APACHE](licenses/paste-1.0.15/LICENSE-APACHE)、[LICENSE-MIT](licenses/paste-1.0.15/LICENSE-MIT) |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/pkg-config-rs) | [LICENSE-APACHE](licenses/pkg-config-0.3.34/LICENSE-APACHE)、[LICENSE-MIT](licenses/pkg-config-0.3.34/LICENSE-MIT) |
| png | 0.18.1 | MIT OR Apache-2.0 | [来源](https://github.com/image-rs/image-png) | [LICENSE-APACHE](licenses/png-0.18.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/png-0.18.1/LICENSE-MIT) |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | [来源](https://github.com/dtolnay/proc-macro2) | [LICENSE-APACHE](licenses/proc-macro2-1.0.107/LICENSE-APACHE)、[LICENSE-MIT](licenses/proc-macro2-1.0.107/LICENSE-MIT) |
| qoi | 0.4.1 | MIT/Apache-2.0 | [来源](https://github.com/aldanor/qoi-rust) | [LICENSE-APACHE](licenses/qoi-0.4.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/qoi-0.4.1/LICENSE-MIT) |
| quick-error | 2.0.1 | MIT/Apache-2.0 | [来源](http://github.com/tailhook/quick-error) | [LICENSE-APACHE](licenses/quick-error-2.0.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/quick-error-2.0.1/LICENSE-MIT) |
| quote | 1.0.47 | MIT OR Apache-2.0 | [来源](https://github.com/dtolnay/quote) | [LICENSE-APACHE](licenses/quote-1.0.47/LICENSE-APACHE)、[LICENSE-MIT](licenses/quote-1.0.47/LICENSE-MIT) |
| regex-lite | 0.1.9 | MIT OR Apache-2.0 | [来源](https://github.com/rust-lang/regex) | [LICENSE-APACHE](licenses/regex-lite-0.1.9/LICENSE-APACHE)、[LICENSE-MIT](licenses/regex-lite-0.1.9/LICENSE-MIT) |
| ruopus | 0.1.2 | MIT | [来源](https://github.com/jmg049/ruopus) | [LICENSE](licenses/ruopus-0.1.2/LICENSE) |
| serde | 1.0.229 | MIT OR Apache-2.0 | [来源](https://github.com/serde-rs/serde) | [LICENSE-APACHE](licenses/serde-1.0.229/LICENSE-APACHE)、[LICENSE-MIT](licenses/serde-1.0.229/LICENSE-MIT) |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | [来源](https://github.com/serde-rs/serde) | [LICENSE-APACHE](licenses/serde_core-1.0.229/LICENSE-APACHE)、[LICENSE-MIT](licenses/serde_core-1.0.229/LICENSE-MIT) |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | [来源](https://github.com/serde-rs/serde) | [LICENSE-APACHE](licenses/serde_derive-1.0.229/LICENSE-APACHE)、[LICENSE-MIT](licenses/serde_derive-1.0.229/LICENSE-MIT) |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | [来源](https://github.com/serde-rs/json) | [LICENSE-APACHE](licenses/serde_json-1.0.154/LICENSE-APACHE)、[LICENSE-MIT](licenses/serde_json-1.0.154/LICENSE-MIT) |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | [来源](https://github.com/RustCrypto/hashes) | [LICENSE-APACHE](licenses/sha2-0.10.9/LICENSE-APACHE)、[LICENSE-MIT](licenses/sha2-0.10.9/LICENSE-MIT) |
| shlex | 2.0.1 | MIT OR Apache-2.0 | [来源](https://github.com/comex/rust-shlex) | [LICENSE-APACHE](licenses/shlex-2.0.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/shlex-2.0.1/LICENSE-MIT) |
| simd-adler32 | 0.3.10 | MIT | [来源](https://github.com/mcountryman/simd-adler32) | [LICENSE.md](licenses/simd-adler32-0.3.10/LICENSE.md) |
| smallvec | 1.16.4 | MIT OR Apache-2.0 | [来源](https://github.com/servo/rust-smallvec) | [LICENSE-APACHE](licenses/smallvec-1.16.4/LICENSE-APACHE)、[LICENSE-MIT](licenses/smallvec-1.16.4/LICENSE-MIT) |
| strsim | 0.11.1 | MIT | [来源](https://github.com/rapidfuzz/strsim-rs) | [LICENSE](licenses/strsim-0.11.1/LICENSE) |
| symphonia | 0.6.1 | MPL-2.0 | [来源](https://github.com/pdeljanov/Symphonia) | [LICENSE](licenses/symphonia-0.6.1/LICENSE) |
| symphonia-bundle-mp3 | 0.6.1 | MPL-2.0 | [来源](https://github.com/pdeljanov/Symphonia) | [LICENSE](licenses/symphonia-bundle-mp3-0.6.1/LICENSE) |
| symphonia-core | 0.6.1 | MPL-2.0 | [来源](https://github.com/pdeljanov/Symphonia) | [LICENSE](licenses/symphonia-core-0.6.1/LICENSE) |
| symphonia-metadata | 0.6.1 | MPL-2.0 | [来源](https://github.com/pdeljanov/Symphonia) | [LICENSE](licenses/symphonia-metadata-0.6.1/LICENSE) |
| syn | 3.0.9 | MIT OR Apache-2.0 | [来源](https://github.com/dtolnay/syn) | [LICENSE-APACHE](licenses/syn-3.0.9/LICENSE-APACHE)、[LICENSE-MIT](licenses/syn-3.0.9/LICENSE-MIT) |
| texture2ddecoder | 0.1.2 | MIT OR Apache-2.0 | [来源](https://github.com/UniversalGameExtraction/texture2ddecoder) | [COPYRIGHT](licenses/texture2ddecoder-0.1.2/COPYRIGHT)、[LICENSE-APACHE](licenses/texture2ddecoder-0.1.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/texture2ddecoder-0.1.2/LICENSE-MIT) |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT | [来源](https://github.com/Lokathor/tinyvec) | [LICENSE-APACHE.md](licenses/tinyvec-1.13.3/LICENSE-APACHE.md)、[LICENSE-MIT.md](licenses/tinyvec-1.13.3/LICENSE-MIT.md)、[LICENSE-ZLIB.md](licenses/tinyvec-1.13.3/LICENSE-ZLIB.md) |
| typed-path | 0.12.4 | MIT OR Apache-2.0 | [来源](https://github.com/chipsenkbeil/typed-path) | [LICENSE-APACHE](licenses/typed-path-0.12.4/LICENSE-APACHE)、[LICENSE-MIT](licenses/typed-path-0.12.4/LICENSE-MIT) |
| typenum | 1.20.1 | MIT OR Apache-2.0 | [来源](https://github.com/paholg/typenum) | [LICENSE](licenses/typenum-1.20.1/LICENSE)、[LICENSE-APACHE](licenses/typenum-1.20.1/LICENSE-APACHE)、[LICENSE-MIT](licenses/typenum-1.20.1/LICENSE-MIT) |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | [来源](https://github.com/dtolnay/unicode-ident) | [LICENSE-APACHE](licenses/unicode-ident-1.0.27/LICENSE-APACHE)、[LICENSE-MIT](licenses/unicode-ident-1.0.27/LICENSE-MIT)、[LICENSE-UNICODE](licenses/unicode-ident-1.0.27/LICENSE-UNICODE) |
| unity-rs-core | 0.6.1 | MIT | [来源](https://github.com/seiunx-dev/unity-rs) | [FSB_VORBIS_NOTICE.md](licenses/unity-rs-core-0.6.1/FSB_VORBIS_NOTICE.md)、[LICENSE](licenses/unity-rs-core-0.6.1/LICENSE)、[THIRD_PARTY_LICENSES.txt](licenses/unity-rs-core-0.6.1/THIRD_PARTY_LICENSES.txt)、[THIRD_PARTY_NOTICES.md](licenses/unity-rs-core-0.6.1/THIRD_PARTY_NOTICES.md)、[src/vendor/texture2ddecoder/COPYRIGHT](licenses/unity-rs-core-0.6.1/src/vendor/texture2ddecoder/COPYRIGHT)、[src/vendor/texture2ddecoder/LICENSE-MIT](licenses/unity-rs-core-0.6.1/src/vendor/texture2ddecoder/LICENSE-MIT) |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT | [来源](https://github.com/alacritty/vte) | [LICENSE-APACHE](licenses/utf8parse-0.2.2/LICENSE-APACHE)、[LICENSE-MIT](licenses/utf8parse-0.2.2/LICENSE-MIT) |
| version_check | 0.9.5 | MIT/Apache-2.0 | [来源](https://github.com/SergioBenitez/version_check) | [LICENSE-APACHE](licenses/version_check-0.9.5/LICENSE-APACHE)、[LICENSE-MIT](licenses/version_check-0.9.5/LICENSE-MIT) |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | [来源](https://github.com/microsoft/windows-rs) | [license-apache-2.0](licenses/windows-link-0.2.1/license-apache-2.0)、[license-mit](licenses/windows-link-0.2.1/license-mit) |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | [来源](https://github.com/microsoft/windows-rs) | [license-apache-2.0](licenses/windows-sys-0.61.2/license-apache-2.0)、[license-mit](licenses/windows-sys-0.61.2/license-mit) |
| zip | 8.6.0 | MIT | [来源](https://github.com/zip-rs/zip2) | [LICENSE](licenses/zip-8.6.0/LICENSE) |
| zlib-rs | 0.6.8 | Zlib | [来源](https://github.com/trifectatechfoundation/zlib-rs) | [LICENSE](licenses/zlib-rs-0.6.8/LICENSE) |
| zmij | 1.0.23 | MIT | [来源](https://github.com/dtolnay/zmij) | [LICENSE-MIT](licenses/zmij-1.0.23/LICENSE-MIT) |
| zstd | 0.14.3 | BSD-3-Clause | [来源](https://github.com/gyscos/zstd-rs) | [LICENSE](licenses/zstd-0.14.3/LICENSE) |
| zstd-safe | 8.2.0 | BSD-3-Clause | [来源](https://github.com/gyscos/zstd-rs) | [LICENSE](licenses/zstd-safe-8.2.0/LICENSE) |
| zstd-sys | 2.1.2+zstd.1.5.7 | BSD-3-Clause | [来源](https://github.com/gyscos/zstd-rs) | [LICENSE](licenses/zstd-sys-2.1.2+zstd.1.5.7/LICENSE)、[LICENSE.BSD-3-Clause](licenses/zstd-sys-2.1.2+zstd.1.5.7/LICENSE.BSD-3-Clause)、[zstd/COPYING](licenses/zstd-sys-2.1.2+zstd.1.5.7/zstd/COPYING)、[zstd/LICENSE](licenses/zstd-sys-2.1.2+zstd.1.5.7/zstd/LICENSE) |
