# Footer Field Overrides 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标:** 在 `WriterProperties` 中新增 footer field override 配置，允许将非标准的 `FileMetaData` field 8（`Bool`）和 field 9（`String`/`Binary`）写入 footer，替代标准的 `EncryptionAlgorithm`/`footer_signing_key_metadata`。

**架构:** 在 `properties.rs` 中新增类型（无 cfg 门控）；在 `build()` 中校验；将 `Option<FooterFieldOverrides>` 存在 `MetadataObjectWriter` 上；写入时通过 `OverrideOutputProtocol`（`TOutputProtocol` 代理）拦截 `write_field_stop`，在生成的 fields 1-7 之后注入 override fields 8-9。通过 `ThriftMetadataWriter` builder 方法透传，在 `SerializedFileWriter::write_metadata` 中接入。

**技术栈:** Rust, Thrift (通过 `parquet-format` 生成的代码), `TCompactOutputProtocol`

**设计文档:** `docs/2026-06-02-footer-field-overrides-design.md`

---

## 文件结构

### 将被修改的文件

| 文件 | 职责 |
|---|---|
| `parquet/src/file/properties.rs` | 新类型 (`FooterFieldOverrides`, `FooterFieldOverride`, `FooterFieldValue`), `WriterProperties`/`WriterPropertiesBuilder` 字段, `build()` 中的校验 |
| `parquet/src/file/metadata/writer.rs` | `MetadataObjectWriter` override 分发, 自定义序列化器 `write_metadata_with_overrides`, `ThriftMetadataWriter` builder 方法 |
| `parquet/src/file/writer.rs` | 将 `footer_field_overrides` 从 `self.props` 传入 `ThriftMetadataWriter` |

### 测试文件

| 文件 | 职责 |
|---|---|
| `parquet/src/file/properties.rs`（已有 test module） | 单元测试：field id / 类型校验，与 `file_encryption_properties` 互斥 |
| 新建集成测试 `parquet/tests/footer_field_overrides/mod.rs` + `parquet/Cargo.toml` | 序列化/行为测试（footer bytes、field 类型验证、与 `external_encryption` 兼容） |

---

## 任务

### Task 1: 在 `properties.rs` 中添加类型和 WriterProperties 接入

**文件:**
- 修改: `parquet/src/file/properties.rs`

- [ ] **Step 1: 在 `WriterProperties` struct 前添加类型定义**

在 ~line 187 后添加（在 `#[derive(Debug, Clone)] pub struct WriterProperties` 之前）：

```rust
use std::collections::BTreeMap;

/// 从 field id 到 footer field override 的映射。
pub type FooterFieldOverrides = BTreeMap<i16, FooterFieldOverride>;

/// 描述 `FileMetaData` thrift 字段的单个 override。
#[derive(Clone, Debug, PartialEq)]
pub struct FooterFieldOverride {
    pub name: String,
    pub value: FooterFieldValue,
}

/// Footer field override 的可能值。
#[derive(Clone, Debug, PartialEq)]
pub enum FooterFieldValue {
    Bool(bool),
    String(String),
    Binary(Vec<u8>),
}
```

- [ ] **Step 2: 在 `WriterProperties` struct 中添加 `footer_field_overrides` 字段**

在 ~line 206 后添加（在 `coerce_types` 之后）：
```rust
    /// Footer field overrides，用于非标准 FileMetaData 字段。
    pub(crate) footer_field_overrides: Option<FooterFieldOverrides>,
```

- [ ] **Step 3: 在 `WriterProperties` impl 中添加 getter 方法**

在 `external_encryption` getter 后添加（~line 458）：
```rust
    /// 返回 footer field overrides（如有）。
    pub fn footer_field_overrides(&self) -> Option<&FooterFieldOverrides> {
        self.footer_field_overrides.as_ref()
    }
```

- [ ] **Step 4: 在 `WriterPropertiesBuilder` struct 中添加 `footer_field_overrides` 字段**

在 ~line 482 后添加（在 `coerce_types` 之后）：
```rust
    footer_field_overrides: Option<FooterFieldOverrides>,
```

在 `Default::default()` 中初始化（~line 498）：
```rust
            footer_field_overrides: None,
```

- [ ] **Step 5: 添加 `with_footer_field_overrides` builder 方法**

在 `external_encryption` builder 方法后添加（~line 786）：
```rust
    /// 设置 footer field overrides（默认为 `None`）。
    ///
    /// 允许写入非标准的 `FileMetaData` fields 8 和 9。
    /// 与 [`file_encryption_properties`](Self::with_file_encryption_properties) 互斥。
    ///
    /// # Panics
    /// 如果 override 包含非 8/9 的 field id，则 panic。
    pub fn with_footer_field_overrides(
        mut self,
        overrides: FooterFieldOverrides,
    ) -> Self {
        self.footer_field_overrides = Some(overrides);
        self
    }
```

- [ ] **Step 6: 在 `build()` 中添加校验 + 复制**

在 `external-encryption` 互斥检查后添加（~line 525）：

```rust
        // 校验 footer_field_overrides：只允许 field id 8 和 9
        if let Some(overrides) = &self.footer_field_overrides {
            for (&id, _) in overrides {
                if id != 8 && id != 9 {
                    panic!(
                        "footer field override for field id {} is not allowed; only 8 and 9 are supported",
                        id
                    );
                }
            }
        }

        // 与 file_encryption_properties 互斥
        #[cfg(feature = "encryption")]
        if self.footer_field_overrides.is_some() && self.file_encryption_properties.is_some() {
            panic!("footer_field_overrides and file_encryption_properties are mutually exclusive");
        }
```

在 `WriterProperties` 构造中添加（~line 543）：
```rust
            footer_field_overrides: self.footer_field_overrides,
```

- [ ] **Step 7: 验证编译**

运行: `cargo check -p parquet --features "arrow" 2>&1 | tail -5`
预期: 编译通过（warnings 可接受）。


### Task 2: 在 `properties.rs` 中添加校验单元测试

- [ ] **Step 8: 添加 field 校验测试**

在 `properties.rs` 已有的 test module 中添加（在 module 结束 `}` 之前）：

```rust
    #[test]
    fn test_footer_field_override_field_8() {
        let overrides = BTreeMap::from([
            (
                8,
                FooterFieldOverride {
                    name: "encrypted".to_string(),
                    value: FooterFieldValue::Bool(true),
                },
            ),
        ]);
        let props = WriterProperties::builder()
            .with_footer_field_overrides(overrides)
            .build();
        assert!(props.footer_field_overrides().is_some());
    }

    #[test]
    fn test_footer_field_override_field_9() {
        let overrides = BTreeMap::from([
            (
                9,
                FooterFieldOverride {
                    name: "keyname".to_string(),
                    value: FooterFieldValue::String("my_key".to_string()),
                },
            ),
        ]);
        let props = WriterProperties::builder()
            .with_footer_field_overrides(overrides)
            .build();
        assert!(props.footer_field_overrides().is_some());
    }

    #[test]
    #[should_panic(expected = "only 8 and 9 are supported")]
    fn test_footer_field_override_invalid_field_id() {
        let overrides = BTreeMap::from([
            (
                10,
                FooterFieldOverride {
                    name: "invalid".to_string(),
                    value: FooterFieldValue::Bool(false),
                },
            ),
        ]);
        WriterProperties::builder()
            .with_footer_field_overrides(overrides)
            .build();
    }
```

添加 `#[cfg(feature = "encryption")]` 互斥测试：

```rust
    #[test]
    #[cfg(feature = "encryption")]
    #[should_panic(expected = "footer_field_overrides and file_encryption_properties are mutually exclusive")]
    fn test_footer_field_override_mutual_exclusion_with_encryption() {
        use crate::file::encryption::FileEncryptionProperties;
        let overrides = BTreeMap::from([
            (
                8,
                FooterFieldOverride {
                    name: "encrypted".to_string(),
                    value: FooterFieldValue::Bool(true),
                },
            ),
        ]);
        let fe_props = FileEncryptionProperties::new(
            crate::file::encryption::EncryptionAlgorithm::AES_GCM_V1,
            None,
        );
        WriterProperties::builder()
            .with_footer_field_overrides(overrides)
            .with_file_encryption_properties(fe_props)
            .build();
    }
```

- [ ] **Step 9: 运行校验测试**

运行: `cargo test -p parquet -- properties::tests::test_footer_field_override 2>&1 | tail -20`
预期: 所有 4 个测试通过。

- [ ] **Step 10: Commit**

```bash
git add parquet/src/file/properties.rs
git commit -m "feat(parquet): add FooterFieldOverrides types and WriterProperties plumbing"
```


### Task 3: 在 `metadata/writer.rs` 中添加 `MetadataObjectWriter` override 支持

**文件:**
- 修改: `parquet/src/file/metadata/writer.rs`

- [ ] **Step 11: 在 `MetadataObjectWriter` struct 中添加 `footer_field_overrides` 字段**

替换 struct 定义（lines 423-427）：
```rust
#[derive(Debug, Default)]
struct MetadataObjectWriter {
    #[cfg(feature = "encryption")]
    file_encryptor: Option<Arc<FileEncryptor>>,
    footer_field_overrides: Option<FooterFieldOverrides>,
}
```

- [ ] **Step 12: 在 `ThriftMetadataWriter` 中添加 `with_footer_field_overrides` 方法**

在 `with_file_encryptor` 方法后添加（~line 225）：
```rust
    pub fn with_footer_field_overrides(
        mut self,
        overrides: Option<FooterFieldOverrides>,
    ) -> Self {
        self.object_writer.footer_field_overrides = overrides;
        self
    }
```

- [ ] **Step 13: 在 `metadata/writer.rs` 顶部添加 import**

```rust
use crate::file::properties::FooterFieldOverrides;
```

- [ ] **Step 14: 添加 `OverrideOutputProtocol` wrapper 和 `write_metadata_with_overrides`**

在 `write_object` helper 后添加（~line 437），在 `#[cfg(not(feature = "encryption"))]` block 之前。

先在文件顶部 imports 追加（`TCompactOutputProtocol` 已导入，只需补剩余类型）：

```rust
use thrift::protocol::{
    TFieldIdentifier, TListIdentifier, TMapIdentifier, TMessageIdentifier,
    TOutputProtocol, TSetIdentifier, TStructIdentifier, TType,
};
```

添加 wrapper struct + `TOutputProtocol` impl：

```rust
    /// 代理 `TOutputProtocol`，写每个字段时都检查是否在 override 中。
    /// 若命中则跳过该字段的标准写入，在 stop 时统一注入 override values。
    struct OverrideOutputProtocol<'a, W: Write> {
        inner: &'a mut TCompactOutputProtocol<W>,
        overrides: &'a FooterFieldOverrides,
        skipping_field_id: Option<i16>,
    }

    impl<'a, W: Write> TOutputProtocol for OverrideOutputProtocol<'a, W> {
        fn write_message_begin(&mut self, id: &TMessageIdentifier<'_>) -> thrift::Result<()> {
            self.inner.write_message_begin(id)
        }
        fn write_message_end(&mut self) -> thrift::Result<()> { self.inner.write_message_end() }
        fn write_struct_begin(&mut self, id: &TStructIdentifier<'_>) -> thrift::Result<()> {
            self.inner.write_struct_begin(id)
        }
        fn write_struct_end(&mut self) -> thrift::Result<()> { self.inner.write_struct_end() }

        fn write_field_begin(&mut self, id: &TFieldIdentifier<'_>) -> thrift::Result<()> {
            if self.overrides.contains_key(&id.id) {
                self.skipping_field_id = Some(id.id);
                return Ok(());
            }
            self.inner.write_field_begin(id)
        }
        fn write_field_end(&mut self) -> thrift::Result<()> {
            if self.skipping_field_id.take().is_some() {
                return Ok(());
            }
            self.inner.write_field_end()
        }

        fn write_field_stop(&mut self) -> thrift::Result<()> {
            let mut field_ids: Vec<i16> = self.overrides.keys().copied().collect();
            field_ids.sort();
            for field_id in field_ids {
                let entry = &self.overrides[&field_id];
                match &entry.value {
                    FooterFieldValue::Bool(v) => {
                        self.inner.write_field_begin(
                            &TFieldIdentifier::new(&entry.name, TType::Bool, field_id))?;
                        self.inner.write_bool(*v)?;
                        self.inner.write_field_end()?;
                    }
                    FooterFieldValue::String(v) => {
                        self.inner.write_field_begin(
                            &TFieldIdentifier::new(&entry.name, TType::String, field_id))?;
                        self.inner.write_string(v)?;
                        self.inner.write_field_end()?;
                    }
                    FooterFieldValue::Binary(v) => {
                        self.inner.write_field_begin(
                            &TFieldIdentifier::new(&entry.name, TType::String, field_id))?;
                        self.inner.write_bytes(v)?;
                        self.inner.write_field_end()?;
                    }
                }
            }
            self.inner.write_field_stop()
        }

        fn write_bool(&mut self, v: bool) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_bool(v)
        }
        fn write_byte(&mut self, v: i8) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_byte(v)
        }
        fn write_i16(&mut self, v: i16) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_i16(v)
        }
        fn write_i32(&mut self, v: i32) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_i32(v)
        }
        fn write_i64(&mut self, v: i64) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_i64(v)
        }
        fn write_double(&mut self, v: f64) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_double(v)
        }
        fn write_string(&mut self, v: &str) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_string(v)
        }
        fn write_bytes(&mut self, v: &[u8]) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_bytes(v)
        }
        fn write_map_begin(&mut self, id: &TMapIdentifier<'_>) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_map_begin(id)
        }
        fn write_map_end(&mut self) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_map_end()
        }
        fn write_list_begin(&mut self, id: &TListIdentifier<'_>) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_list_begin(id)
        }
        fn write_list_end(&mut self) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_list_end()
        }
        fn write_set_begin(&mut self, id: &TSetIdentifier<'_>) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_set_begin(id)
        }
        fn write_set_end(&mut self) -> thrift::Result<()> {
            if self.skipping_field_id.is_some() { return Ok(()); }
            self.inner.write_set_end()
        }
        fn flush(&mut self) -> thrift::Result<()> { self.inner.flush() }
    }
```

- [ ] **Step 15: 修改 `write_file_metadata`，添加 override 分发 + helper**

在无加密路径中，替换 `write_file_metadata`（lines 442-444）：

```rust
    fn write_file_metadata(&self, file_metadata: &FileMetaData, sink: impl Write) -> Result<()> {
        match &self.footer_field_overrides {
            Some(overrides) => {
                Self::write_metadata_with_overrides(file_metadata, overrides, sink)
            }
            None => Self::write_object(file_metadata, sink),
        }
    }
```

同步添加 `write_metadata_with_overrides` helper：

```rust
    fn write_metadata_with_overrides(
        file_metadata: &FileMetaData,
        overrides: &FooterFieldOverrides,
        sink: impl Write,
    ) -> Result<()> {
        let mut protocol = TCompactOutputProtocol::new(sink);
        let mut wrapper = OverrideOutputProtocol {
            inner: &mut protocol,
            overrides,
        };
        file_metadata.write_to_out_protocol(&mut wrapper)?;
        Ok(())
    }
```

同时更新加密路径的 `write_file_metadata`（line 522）的 fallthrough 分支：

```rust
            _ => match &self.footer_field_overrides {
                Some(overrides) => {
                    Self::write_metadata_with_overrides(file_metadata, overrides, &mut sink)
                }
                None => Self::write_object(file_metadata, &mut sink),
            },
```

- [ ] **Step 16: 验证编译**

运行: `cargo check -p parquet --features "arrow,external-encryption" 2>&1 | tail -5`
预期: 编译通过。

- [ ] **Step 17: Commit**

```bash
git add parquet/src/file/metadata/writer.rs
git commit -m "feat(parquet): add custom FileMetaData serializer with field overrides"
```


### Task 4: 通过 `SerializedFileWriter` 接入 `footer_field_overrides`

**文件:**
- 修改: `parquet/src/file/writer.rs`

- [ ] **Step 18: 将 `footer_field_overrides` 传入 `ThriftMetadataWriter`**

在 `write_metadata`（~line 342）中，在 `ThriftMetadataWriter::new(...)` 调用之后、`#[cfg(feature = "encryption")]` block 之前添加：

```rust
        encoder = encoder.with_footer_field_overrides(
            self.props.footer_field_overrides().cloned(),
        );
```

加密路径的 `write_file_metadata` 修改已在 Step 15 中完成。

- [ ] **Step 19: 验证所有 feature 组合编译**

运行:
```
cargo check -p parquet --features "arrow,external-encryption" 2>&1 | tail -5
cargo check -p parquet --features "arrow,encryption" 2>&1 | tail -5
cargo check -p parquet --features "arrow,encryption,external-encryption" 2>&1 | tail -5
```
预期: 全部编译通过。

- [ ] **Step 20: Commit**

```bash
git add parquet/src/file/writer.rs
git commit -m "feat(parquet): wire footer_field_overrides through SerializedFileWriter"
```


### Task 5: 集成测试

**文件:**
- 创建: `parquet/tests/footer_field_overrides/mod.rs`
- 修改: `parquet/Cargo.toml`

- [ ] **Step 21: 创建测试文件**

```rust
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use bytes::Bytes;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::{
    FooterFieldOverride, FooterFieldOverrides, FooterFieldValue, WriterProperties,
};
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;

fn make_batch() -> RecordBatch {
    let col = Arc::new(Int64Array::from_iter_values([1, 2, 3])) as ArrayRef;
    RecordBatch::try_from_iter([("col", col)]).unwrap()
}

fn write_file(overrides: Option<FooterFieldOverrides>) -> Vec<u8> {
    let batch = make_batch();
    let schema = batch.schema();
    let mut buf = Vec::new();

    let mut builder = WriterProperties::builder();
    if let Some(o) = overrides {
        builder = builder.with_footer_field_overrides(o);
    }
    let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(builder.build())).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    buf
}

#[test]
fn test_footer_field_override_field_8_9_serialized() {
    let overrides = BTreeMap::from([
        (
            8,
            FooterFieldOverride {
                name: "encrypted".to_string(),
                value: FooterFieldValue::Bool(true),
            },
        ),
        (
            9,
            FooterFieldOverride {
                name: "keyname".to_string(),
                value: FooterFieldValue::String("my_key".to_string()),
            },
        ),
    ]);
    let buffer = write_file(Some(overrides));

    // 验证 PAR1 magic
    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");

    // 标准 reader 应能解析 metadata（fields 1-7 是标准格式）
    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    let file_meta = reader.metadata().file_metadata();
    assert_eq!(file_meta.num_rows(), 3);
}

#[test]
fn test_footer_field_override_without_external_encryption() {
    // Footer override 单独使用，不依赖 external_encryption
    let overrides = BTreeMap::from([
        (
            8,
            FooterFieldOverride {
                name: "encrypted".to_string(),
                value: FooterFieldValue::Bool(true),
            },
        ),
    ]);
    let buffer = write_file(Some(overrides));

    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 3);
}

#[test]
fn test_footer_no_overrides_unchanged() {
    // 无 override 时，footer bytes 应与标准路径一致
    let buffer_default = write_file(None);
    let buffer_default2 = write_file(None);
    assert_eq!(buffer_default, buffer_default2);
}
```

- [ ] **Step 22: 在 `Cargo.toml` 中注册测试**

在 `parquet/Cargo.toml` 的 `[[test]]` 配置中添加：

```toml
[[test]]
name = "footer_field_overrides"
path = "tests/footer_field_overrides/mod.rs"
required-features = ["arrow"]
```

- [ ] **Step 23: 运行集成测试**

运行: `cargo test --test footer_field_overrides -p parquet --features "arrow" 2>&1 | tail -10`
预期: 3 个测试通过。

运行: `cargo test --test footer_field_overrides -p parquet --features "arrow,external-encryption" 2>&1 | tail -10`
预期: 3 个测试通过。

- [ ] **Step 24: 验证现有测试仍然通过**

运行: `cargo test -p parquet --features "arrow" 2>&1 | tail -5`

运行: `cargo test -p parquet --features "arrow,encryption" 2>&1 | tail -5`

- [ ] **Step 25: Commit**

```bash
git add parquet/Cargo.toml parquet/tests/footer_field_overrides/mod.rs
git commit -m "test(parquet): add integration tests for footer field overrides"
```


### Task 6: 处理加密路径的 override（同时启用 `encryption` + `external-encryption`）

- [ ] **Step 26: 验证加密路径正确处理 overrides**

`#[cfg(feature = "encryption")]` 的 `write_file_metadata` 已经在 `_ =>` 分支中检查了 `self.footer_field_overrides`（来自 Step 15）。验证两个 feature 同时编译：
运行: `cargo check -p parquet --features "arrow,encryption,external-encryption" 2>&1 | tail -5`

- [ ] **Step 27: 如果有需要，最终 commit**

```bash
git add parquet/src/file/metadata/writer.rs
git commit -m "fix(parquet): handle footer overrides in encryption-path MetadataObjectWriter"
```


## 自审清单

1. **Spec 覆盖:**
   - FooterFieldOverrides 类型: ✅ Task 1
   - WriterProperties 接入: ✅ Task 1
   - 校验（仅 8/9、互斥）: ✅ Task 1
   - MetadataObjectWriter 字段 + 分发: ✅ Task 3
   - 自定义序列化器 write_metadata_with_overrides: ✅ Task 3
   - 通过 SerializedFileWriter 接入: ✅ Task 4
   - 集成测试: ✅ Task 5
   - 不修改 format.rs: ✅ (已确认)
   - 不依赖 external_encryption 单独使用: ✅ Task 5 测试

2. **占位符检查:** 无 "TBD"、"TODO" 或缺失代码。

3. **类型一致性:** 所有类型在各 task 间一致。`FooterFieldOverrides` = `BTreeMap<i16, FooterFieldOverride>`。`FooterFieldValue` 包含 `Bool`、`String`、`Binary` 变体。
