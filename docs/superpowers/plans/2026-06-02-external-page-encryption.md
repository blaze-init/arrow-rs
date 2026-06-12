# External Page Encryption Implementation Plan

**Goal:** 新增 `external-encryption` feature, 让 `ArrowWriter` 使用方注入 page data 加密函数, 只加密 page data (#1), 其他 8 个加密点全部明文.

**Reference:** `docs/2026-06-02-external-page-encryption-design.md`

**改动汇总:** 5 个文件, ~200 行净增

---

### 任务 1: `parquet/Cargo.toml` — 新增 feature

**修改:**
- `parquet/Cargo.toml:121` (features 段)

**改动:**
```toml
encryption = ["dep:ring"]
external-encryption = []
```

不依赖 `encryption`, 不拉 `dep:ring`.

---

### 任务 2: `parquet/src/file/properties.rs` — 公开类型 + WriterProperties 字段

#### 2a: 新增 `ExternalEncryptFn` + `ExternalEncryption`

在 `use crate::encryption::encrypt::FileEncryptionProperties;` (line 22) 之后插入:

```rust
#[cfg(feature = "external-encryption")]
pub type ExternalEncryptFn = Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>;

#[cfg(feature = "external-encryption")]
#[derive(Clone)]
pub struct ExternalEncryption {
    encryptor: ExternalEncryptFn,
}

#[cfg(feature = "external-encryption")]
impl std::fmt::Debug for ExternalEncryption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalEncryption")
            .field("encryptor", &"<fn>")
            .finish()
    }
}

#[cfg(feature = "external-encryption")]
impl ExternalEncryption {
    pub fn new(encryptor: ExternalEncryptFn) -> Self {
        Self { encryptor }
    }

    pub(crate) fn encrypt_page(&self, data: &[u8]) -> Result<Vec<u8>> {
        (self.encryptor)(data)
    }
}
```

#### 2b: `WriterProperties` 新增字段 + getter

在 `file_encryption_properties` 字段 (line 175) 之后:

```rust
#[cfg(feature = "encryption")]
pub(crate) file_encryption_properties: ...,
#[cfg(feature = "external-encryption")]
external_encryption: Option<ExternalEncryption>,
```

在 `file_encryption_properties()` getter (line 418-419) 之后:

```rust
#[cfg(feature = "encryption")]
pub fn file_encryption_properties(&self) -> Option<&FileEncryptionProperties> {
    self.file_encryption_properties.as_ref()
}

#[cfg(feature = "external-encryption")]
pub fn external_encryption(&self) -> Option<&ExternalEncryption> {
    self.external_encryption.as_ref()
}
```

#### 2c: `WriterPropertiesBuilder` 新增字段 + builder 方法

字段 (line 443-444):
```rust
#[cfg(feature = "encryption")]
file_encryption_properties: Option<FileEncryptionProperties>,
#[cfg(feature = "external-encryption")]
external_encryption: Option<ExternalEncryption>,
```

`with_defaults` (line 467-468):
```rust
#[cfg(feature = "encryption")]
file_encryption_properties: None,
#[cfg(feature = "external-encryption")]
external_encryption: None,
```

在 `with_file_encryption_properties` (line 709) 之后新增:
```rust
#[cfg(feature = "external-encryption")]
pub fn with_external_encryption(
    mut self,
    encryption: ExternalEncryption,
) -> Self {
    self.external_encryption = Some(encryption);
    self
}
```

#### 2d: `build()` 互斥校验 + 字段赋值

```rust
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
        #[cfg(feature = "encryption")]
        file_encryption_properties: self.file_encryption_properties,
        #[cfg(feature = "external-encryption")]
        external_encryption: self.external_encryption,
    }
}
```

---

### 任务 3: `parquet/src/arrow/arrow_writer/mod.rs` — ArrowPageWriter 织入

#### 3a: 新增 import

在 `use crate::column::page_encryption::PageEncryptor;` (line 38) 之后:
```rust
#[cfg(feature = "external-encryption")]
use crate::file::properties::ExternalEncryption;
```

#### 3b: `ArrowPageWriter` 字段 + builder

```rust
struct ArrowPageWriter {
    buffer: SharedColumnChunk,
    #[cfg(feature = "encryption")]
    page_encryptor: Option<PageEncryptor>,
    #[cfg(feature = "external-encryption")]
    external_encryption: Option<ExternalEncryption>,
}

impl ArrowPageWriter {
    #[cfg(feature = "encryption")]
    pub fn with_encryptor(mut self, page_encryptor: Option<PageEncryptor>) -> Self { ... }

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

#### 3c: `write_page` 插入加密分支

当前:
```rust
fn write_page(&mut self, page: CompressedPage) -> Result<PageWriteSpec> {
    let page = match self.page_encryptor_mut() {
        Some(page_encryptor) => page_encryptor.encrypt_compressed_page(page)?,
        None => page,
    };

    let page_header = page.to_thrift_header();
    // ...
```

改为:
```rust
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
    // ...
```

插入点在 `to_thrift_header()` 之前, 这样 header 里的 compressed_size 基于加密后的实际 bytes.

#### 3d: `ArrowColumnWriterFactory` 增加 external_encryption

```rust
struct ArrowColumnWriterFactory {
    #[cfg(feature = "encryption")]
    row_group_index: usize,
    #[cfg(feature = "encryption")]
    file_encryptor: Option<Arc<FileEncryptor>>,
    #[cfg(feature = "external-encryption")]
    external_encryption: Option<ExternalEncryption>,
}

impl ArrowColumnWriterFactory {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "encryption")]
            row_group_index: 0,
            #[cfg(feature = "encryption")]
            file_encryptor: None,
            #[cfg(feature = "external-encryption")]
            external_encryption: None,
        }
    }

    #[cfg(feature = "encryption")]
    pub fn with_file_encryptor(...) -> Self { ... }

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

#### 3e: `create_page_writer` 改为一个分支

把原来 `encryption` 版本的范围从 `#[cfg(feature = "encryption")]` 改为 `#[cfg(any(...))]`, 内部条件编译 logic:

```rust
#[cfg(any(feature = "encryption", feature = "external-encryption"))]
fn create_page_writer(
    &self,
    column_descriptor: &ColumnDescPtr,
    column_index: usize,
) -> Result<Box<ArrowPageWriter>> {
    #[cfg(feature = "encryption")]
    let page_encryptor = {
        let column_path = column_descriptor.path().string();
        PageEncryptor::create_if_column_encrypted(
            &self.file_encryptor,
            self.row_group_index,
            column_index,
            &column_path,
        )?
    };

    #[cfg(not(feature = "encryption"))]
    let _ = (column_descriptor, column_index);

    let page_writer = ArrowPageWriter::default();
    #[cfg(feature = "encryption")]
    let page_writer = page_writer.with_encryptor(page_encryptor);
    #[cfg(feature = "external-encryption")]
    let page_writer = page_writer.with_external_encryption(self.external_encryption.clone());
    Ok(Box::new(page_writer))
}
```

`#[cfg(not(any(feature = "encryption", feature = "external-encryption")))]` 版本原封不动:

```rust
#[cfg(not(any(feature = "encryption", feature = "external-encryption")))]
fn create_page_writer(
    &self,
    _column_descriptor: &ColumnDescPtr,
    _column_index: usize,
) -> Result<Box<ArrowPageWriter>> {
    Ok(Box::<ArrowPageWriter>::default())
}
```

#### 3f: `get_column_writers` + `get_column_writers_with_encryptor`

`get_column_writers`:
```rust
pub fn get_column_writers(...) -> Result<Vec<ArrowColumnWriter>> {
    // ...
    let column_factory = ArrowColumnWriterFactory::new();
    #[cfg(feature = "external-encryption")]
    let column_factory = column_factory.with_external_encryption(props.external_encryption().cloned());
    // ...
}
```

`get_column_writers_with_encryptor` (已经 `#[cfg(feature = "encryption")]`):
```rust
fn get_column_writers_with_encryptor(...) -> Result<Vec<ArrowColumnWriter>> {
    // ...
    let column_factory =
        ArrowColumnWriterFactory::new().with_file_encryptor(row_group_index, file_encryptor);
    #[cfg(feature = "external-encryption")]
    let column_factory = column_factory.with_external_encryption(props.external_encryption().cloned());
    // ...
}
```

---

### 任务 4: 集成测试

**新建:** `parquet/tests/external_encryption/mod.rs`

```rust
use std::sync::Arc;
use bytes::Bytes;
use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::errors::Result;
use parquet::file::properties::{ExternalEncryption, ExternalEncryptFn, WriterProperties};
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;

fn xor_encrypt(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.iter().map(|b| b ^ 0xFF).collect())
}

fn encrypt_fn() -> ExternalEncryptFn {
    Arc::new(xor_encrypt)
}

fn make_batch() -> RecordBatch {
    let col = Arc::new(Int64Array::from_iter_values([1, 2, 3])) as ArrayRef;
    RecordBatch::try_from_iter([("col", col)]).unwrap()
}

fn write_batch(batch: &RecordBatch, props: Option<WriterProperties>) -> Vec<u8> {
    let schema = batch.schema();
    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, schema, props.map(Arc::new)).unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
    buf
}

#[test]
fn test_par1_magic() {
    let buffer = write_batch(&make_batch(), Some(
        WriterProperties::builder()
            .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
            .build(),
    ));
    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");
}

#[test]
fn test_plaintext_metadata() {
    let buffer = write_batch(&make_batch(), Some(
        WriterProperties::builder()
            .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
            .build(),
    ));
    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    let file_meta = reader.metadata().file_metadata();
    assert_eq!(file_meta.num_rows(), 3);
}

#[test]
fn test_no_crypto_metadata() {
    let buffer = write_batch(&make_batch(), Some(
        WriterProperties::builder()
            .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
            .build(),
    ));
    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    for rg in reader.metadata().row_groups() {
        for col in 0..rg.num_columns() {
            assert!(rg.column(col).crypto_metadata().is_none());
        }
    }
}
```

**`parquet/Cargo.toml`** 注册:
```toml
[[test]]
name = "external_encryption"
required-features = ["arrow", "external-encryption"]
```

---

### 任务 5: Feature 编译验证

```bash
# 无 features
cargo build -p parquet --no-default-features
# 标准加密
cargo build -p parquet --features encryption
# 仅 external
cargo build -p parquet --no-default-features --features external-encryption
# 共存
cargo build -p parquet --features encryption,external-encryption
# 完整测试 (含集成测试)
cargo test -p parquet --no-default-features --features external-encryption,arrow
```

---

### 不改的文件 (确认)

- `parquet/src/column/page_encryption.rs` — 不变
- `parquet/src/file/writer.rs` — 不变
- `parquet/src/file/metadata/writer.rs` — 不变
- `parquet/src/column/mod.rs` — 不变
- `parquet/src/lib.rs` — 不变
