# External Page Encryption 设计方案

- **日期**: 2026-06-02
- **状态**: 待评审
- **作者**: Codex
- **适用范围**: `parquet` crate 的 `ArrowWriter` 写入路径

## 1. 背景与目标

### 1.1 背景

`parquet` crate 当前通过 `feature = "encryption"` 实现 Parquet modular encryption, 由 `FileEncryptor` 驱动以下 9 个标准加密点:

| # | 位置 | 加密对象 |
|---|---|---|
| 1 | `column/page_encryption.rs::encrypt_page` | page data |
| 2 | `column/page_encryption.rs::encrypt_page_header` | page header (Thrift) |
| 3 | `file/metadata/writer.rs::write_file_metadata` | footer (`FileMetaData`) |
| 4 | `file/metadata/writer.rs::write_offset_index` | `OffsetIndex` |
| 5 | `file/metadata/writer.rs::write_column_index` | `ColumnIndex` |
| 6 | `file/metadata/writer.rs::encrypt_row_groups` | column chunk 的 `meta_data` |
| 7 | `file/metadata/writer.rs::get_plaintext_footer_crypto_metadata` | footer 中的 `EncryptionAlgorithm` / `key_metadata` |
| 8 | `file/metadata/writer.rs::get_file_magic` / `file/writer.rs::start_file` | 4 字节 magic (`PAR1` / `PARE`) |
| 9 | `file/writer.rs::set_column_crypto_metadata` / `arrow/arrow_writer/mod.rs` | column chunk 的 `crypto_metadata` 字段 |

### 1.2 目标

新增 cargo feature `external-encryption`, 让下游使用方:

- 通过公开 API 注入一个 page data 加密函数
- 加密函数只接收 page data bytes, 不接收 AAD
- **只加密 page data** (#1)
- 其他 8 个标准加密点 (#2 - #9) **全部保持明文**, 走标准 Parquet 文件结构
- `parquet` crate **不引入**使用方加密库的依赖
- `external-encryption` 与标准 `encryption` feature 可以在同一个 build 中共存, 不互相改变语义

### 1.3 非目标

- 读取路径不实现. 写出的文件 footer 是明文且无 `EncryptionAlgorithm` 标记, 标准 reader 走明文路径读取 page data, 得到的密文由使用方自行解密.
- 不替代标准 Parquet modular encryption. 标准模式 (`feature = "encryption"` + `FileEncryptionProperties`) 行为完全不变.
- 不实现 Parquet modular encryption 的 AAD 语义. external 模式只是对 page data bytes 做外部变换.
- 暂不改低层 `SerializedFileWriter` / `SerializedPageWriter` 直接写入路径. 本方案只覆盖使用方当前需要的 `ArrowWriter`.

## 2. 设计

### 2.1 公开 API

external page encryption 不放进 `FileEncryptionProperties`. `FileEncryptionProperties` 仍然只表示标准 Parquet modular encryption.

新增类型放在 `parquet/src/file/properties.rs`, 随 `feature = "external-encryption"` 暴露:

```rust
/// 自定义 page data 加密函数签名.
///
/// 参数是 CompressedPage 的 data bytes, 返回写入文件的 ciphertext bytes.
/// parquet 不传入 AAD, nonce, key metadata 或任何加密上下文.
pub type ExternalEncryptFn =
    Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>;

/// 自定义 page data 加密配置.
#[derive(Clone)]
pub struct ExternalEncryption {
    encrypt_fn: ExternalEncryptFn,
}

impl std::fmt::Debug for ExternalEncryption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalEncryption")
            .field("encrypt_fn", &"<fn>")
            .finish()
    }
}

impl ExternalEncryption {
    pub fn new(encrypt_fn: ExternalEncryptFn) -> Self {
        Self { encrypt_fn }
    }

    pub(crate) fn encrypt_page(&self, data: &[u8]) -> Result<Vec<u8>> {
        (self.encrypt_fn)(data)
    }
}
```

`WriterProperties` 增加独立字段:

```rust
pub struct WriterProperties {
    // ... 现有字段保持不变

    #[cfg(feature = "external-encryption")]
    external_encryption: Option<ExternalEncryption>,
}

impl WriterProperties {
    #[cfg(feature = "external-encryption")]
    pub fn external_encryption(&self) -> Option<&ExternalEncryption> {
        self.external_encryption.as_ref()
    }
}

pub struct WriterPropertiesBuilder {
    // ... 现有字段保持不变

    #[cfg(feature = "external-encryption")]
    external_encryption: Option<ExternalEncryption>,
}

impl WriterPropertiesBuilder {
    #[cfg(feature = "external-encryption")]
    pub fn with_external_encryption(
        mut self,
        encryption: ExternalEncryption,
    ) -> Self {
        self.external_encryption = Some(encryption);
        self
    }

    pub fn build(self) -> WriterProperties {
        #[cfg(all(feature = "encryption", feature = "external-encryption"))]
        if self.file_encryption_properties.is_some()
            && self.external_encryption.is_some()
        {
            panic!(
                "file_encryption_properties and external_encryption are mutually exclusive"
            );
        }

        WriterProperties {
            // ... 现有字段
            #[cfg(feature = "external-encryption")]
            external_encryption: self.external_encryption,
        }
    }
}
```

互斥检查放在 `WriterPropertiesBuilder::build()` 是因为两个配置都挂在 writer 级别:

- `file_encryption_properties`: 标准 Parquet modular encryption
- `external_encryption`: 只加密 page data 的外部函数

如果需要避免 `build()` panic, 可以把现有 `build()` 保持不变, 另加 `try_build() -> Result<WriterProperties>`. 具体实现阶段按当前 API 兼容性取舍.

### 2.2 标准加密路径保持不变

本方案不改造现有标准 `PageEncryptor`, 不把它 trait 化, 也不改低层 `SerializedPageWriter`.

`get_file_encryptor`, `PageEncryptor`, `MetadataObjectWriter`, `set_column_crypto_metadata`, `get_file_magic` 的标准加密逻辑保持 `#[cfg(feature = "encryption")]`, 不写 `not(feature = "external-encryption")`.

如果只启用 `external-encryption` 而不启用 `encryption`, 标准 Parquet modular encryption 代码不会编译进来, 也不会发生 footer/header/index/metadata 加密.

如果两个 feature 同时启用, `WriterPropertiesBuilder::build()` 仍然禁止同一个 writer 同时配置 `file_encryption_properties` 和 `external_encryption`.

external 模式不设置 `file_encryption_properties`, 因此:

- `get_file_encryptor` 返回 `None`
- footer 明文
- OffsetIndex / ColumnIndex 明文
- row group metadata 不加密
- footer 不写 `EncryptionAlgorithm`
- file magic 是 `PAR1`
- column chunk 不写 `crypto_metadata`

这比用 cfg 排除标准代码更安全: 即使 `encryption` 和 `external-encryption` 同时启用, 运行时也只由 writer properties 决定当前文件走哪种模式.

### 2.3 ArrowWriter 介入点

`ArrowWriter` 虽然持有 `SerializedFileWriter`, 但 page 写入使用自己的 `ArrowPageWriter::write_page`, 不会走低层 `SerializedPageWriter::write_page`. 因此当前只需要改 `parquet/src/arrow/arrow_writer/mod.rs`.

在 `ArrowPageWriter` 上增加 external 配置:

```rust
#[derive(Default)]
struct ArrowPageWriter {
    buffer: SharedColumnChunk,

    #[cfg(feature = "encryption")]
    page_encryptor: Option<PageEncryptor>,

    #[cfg(feature = "external-encryption")]
    external_encryption: Option<ExternalEncryption>,
}

impl ArrowPageWriter {
    #[cfg(feature = "external-encryption")]
    pub fn with_external_encryption(
        mut self,
        external_encryption: Option<ExternalEncryption>,
    ) -> Self {
        self.external_encryption = external_encryption;
        self
    }
}
```

在 `ArrowPageWriter::write_page` 中, 仿照现有标准 page encryptor 的分支, 在生成 page header 前插入 external 加密:

```rust
impl PageWriter for ArrowPageWriter {
    fn write_page(&mut self, page: CompressedPage) -> Result<PageWriteSpec> {
        let page = match self.page_encryptor_mut() {
            Some(page_encryptor) => page_encryptor.encrypt_compressed_page(page)?,
            None => page,
        };

        #[cfg(feature = "external-encryption")]
        let page = match self.external_encryption.as_ref() {
            Some(external) => {
                let encrypted = external.encrypt_page(page.data())?;
                page.with_new_compressed_buffer(Bytes::from(encrypted))
            }
            None => page,
        };

        let page_header = page.to_thrift_header();

        // 后续现有逻辑保持不变:
        // - page header 明文写入
        // - encrypted page data 写入 buffer
        // - PageWriteSpec 使用替换后的 page buffer 长度
    }
}
```

插入点必须在 `page.to_thrift_header()` 之前, 这样 header 中的 compressed size 和后续 column metadata 都基于外部加密后的实际 bytes.

因为 `WriterPropertiesBuilder::build()` 已经互斥校验, 正常配置下不会出现标准 page encryption 和 external page encryption 同时执行. 上面的顺序只是保持代码局部简单, 并让 feature 同时启用时仍然有确定行为.

`ArrowColumnWriterFactory` 负责从 `SerializedFileWriter` 的 `WriterProperties` 读取 `external_encryption`, 并在 `create_page_writer` 时传给 `ArrowPageWriter`.

### 2.4 Feature 关系

`external-encryption` 是独立 feature, 不自动启用 `encryption`:

```toml
[features]
default = []
encryption = ["dep:ring"]
external-encryption = []
```

external API 放在 `file::properties`, 不依赖 `encryption` module, 也不需要改 `lib.rs` 的 `experimental!(pub mod encryption)` gate. 核心要求是 `external-encryption` 不应拉入 `dep:ring`.

| 用户启用 | 行为 |
|---|---|
| 无 | 不支持标准加密, 不支持 external page encryption |
| `encryption` | 支持标准 Parquet modular encryption |
| `external-encryption` | 支持 `ArrowWriter` external page data encryption, 不拉 `ring` |
| `encryption,external-encryption` | 两套 API 都可用; 单个 writer 配置上互斥 |

### 2.5 行为表

启用 `external-encryption` 且配置 `WriterProperties::with_external_encryption(...)` 后:

| 加密点 (#) | 行为 |
|---|---|
| 1. page data | 走用户注入的 `encrypt_fn(&[u8])` |
| 2. page header | 明文 Thrift header |
| 3. footer (`FileMetaData`) | 明文 |
| 4. OffsetIndex | 明文 |
| 5. ColumnIndex | 明文 |
| 6. row group meta_data | 明文 |
| 7. footer crypto metadata | 不写 |
| 8. file magic | 标准 `PAR1` |
| 9. column crypto_metadata | 不写 |

## 3. 改动文件清单

1. **`parquet/Cargo.toml`**
   - 新增 `external-encryption = []`
   - 不依赖 `encryption`, 不拉 `dep:ring`

2. **`parquet/src/file/properties.rs`**
   - 新增 `ExternalEncryptFn`
   - 新增 `ExternalEncryption`
   - `WriterProperties` / `WriterPropertiesBuilder` 增加 `external_encryption`
   - `WriterPropertiesBuilder` 增加 `with_external_encryption`
   - 增加标准加密与 external 加密的互斥校验

3. **`parquet/src/arrow/arrow_writer/mod.rs`**
   - `ArrowPageWriter` 增加 `external_encryption: Option<ExternalEncryption>`
   - `ArrowPageWriter::write_page` 在 `page.to_thrift_header()` 前插入 external page data 加密分支
   - `ArrowColumnWriterFactory` 保存 `external_encryption`
   - `create_page_writer` 把 `external_encryption` 传给 `ArrowPageWriter`
   - 现有标准 `page_encryptor` 逻辑保持不变

4. **无需修改的标准加密文件**
   - `parquet/src/column/page_encryption.rs` 保持不变
   - `parquet/src/file/writer.rs` 保持不变
   - `parquet/src/file/metadata/writer.rs` 保持不变
   - `parquet/src/column/mod.rs` 保持不变

## 4. 读取路径 (非目标)

不实现. 写出的文件:

- footer 明文
- 无 `EncryptionAlgorithm` 标记
- file magic 是标准 `PAR1`
- page header 明文
- page data 是用户函数返回的 bytes

标准 reader 会按明文 Parquet 文件解析 metadata 和 page header, 但读到的 page data 是密文. 如果密文不再是合法压缩后的 page payload, 标准 reader 在解码 page data 时会失败. 使用方需要在读取路径自行解密 page data 后再交给 Parquet 解码.

未来可对称实现 `with_external_page_decryption(...)`, 不在本方案范围.

## 5. 测试策略

1. **单元测试**
   - `ExternalEncryption::encrypt_page`: mock 函数收到原始 `page.data()`, 返回值成为新的 page buffer
   - `WriterPropertiesBuilder`: 同时配置标准 `file_encryption_properties` 和 `external_encryption` 时失败

2. **集成测试**
   - `ArrowWriter` 启用 `external-encryption` 写出文件
   - 验证起始和结尾 magic 都是 `PAR1`
   - 验证 footer 明文, 无 `FileCryptoMetaData`
   - 验证 `column.crypto_metadata` 为空
   - 验证 page header 可正常解析
   - 验证 page data 区域是注入函数返回的 bytes

3. **feature 编译测试**
   - `cargo build -p parquet --no-default-features`
   - `cargo build -p parquet --features encryption`
   - `cargo build -p parquet --features external-encryption`
   - `cargo build -p parquet --features encryption,external-encryption`

4. **互斥参数测试**
   - 同时配置 `with_file_encryption_properties(...)` 和 `with_external_encryption(...)` 时失败
   - 只配置标准 encryption 时, 标准 Parquet modular encryption 行为不变
   - 只配置 external encryption 时, metadata/magic/header 全部保持明文

## 6. 风险与限制

| # | 项 | 说明 |
|---|---|---|
| 1 | 文件不可自描述 | 文件是 `PAR1` 且无 encryption metadata, 读侧无法仅凭文件识别 page data 已被外部加密. 使用方需要外部约定. |
| 2 | 标准 reader 不能直接解码密文 page data | 标准 reader 可解析 footer/page header, 但 page data 解码会失败或得到无意义 bytes. |
| 3 | 加密函数不接收上下文 | external 模式不提供 AAD/page index/column path. 如果使用方需要这些上下文, 需要另设计 API. |
| 4 | 输出大小可能变化 | 加密函数返回的 ciphertext 长度可能不同于 plaintext. writer 必须使用替换后的 buffer 长度更新 page header 和 column metadata. |
| 5 | API 入口变化 | external 模式放在 `WriterProperties`, 不放在 `FileEncryptionProperties`, 避免和标准 modular encryption 混淆. |
| 6 | 只覆盖 ArrowWriter | 低层 `SerializedFileWriter` 直接写入路径不会执行 external encryption. 如果未来需要, 再按同样思路在 `SerializedPageWriter::write_page` 增加独立 hook. |
| 7 | 读取路径缺失 | 本方案只覆盖写入. 真正端到端使用需要外部 reader shim 或后续新增解密 API. |

## 7. 后续工作 (Out of Scope)

- 对称地实现 `with_external_page_decryption` 读取 API
- 提供一个最小示例, 演示如何接入使用方自己的加密/解密库
- 在官方文档中加一节 "External Page Encryption" 介绍

## 8. 决策记录

| 选项 | 决策 | 理由 |
|---|---|---|
| API 入口: `FileEncryptionProperties` vs `WriterProperties` | **`WriterProperties`** | external 模式不是 Parquet modular encryption, 不应该触发 footer/magic/crypto metadata 语义. |
| 注入函数签名 | **`Fn(&[u8]) -> Result<Vec<u8>>`** | 使用方确认不需要 AAD. API 只表达 page data bytes 变换. |
| Feature 关系: 父子 vs 独立 | **独立 feature** | 避免 Cargo feature 合并时 `external-encryption` 改变标准 `encryption` 用户的行为, 同时避免无意义拉入 `ring`. |
| 分发方式: cfg 排除 vs 运行时互斥 | **运行时互斥** | 两套代码可以同编译; 单个 writer 配置只允许一种模式. |
| 介入点 | **`ArrowPageWriter::write_page` 中的独立 match 分支** | 使用方只需要 `ArrowWriter`; 不改标准 `PageEncryptor`, 不抽 helper, 改动面最小. |
