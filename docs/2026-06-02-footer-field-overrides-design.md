# Footer Field Overrides 设计方案

- **日期**: 2026-06-02
- **状态**: 待评审
- **作者**: OpenCode
- **适用范围**: `parquet` crate footer (`FileMetaData`) 写入路径

## 1. 背景与目标

### 1.1 背景

当前 `parquet` crate 的 footer 使用 Thrift `FileMetaData` 写出. 标准 Parquet schema 中, `FileMetaData` 的 field 8 / 9 是:

```thrift
8: optional EncryptionAlgorithm encryption_algorithm
9: optional binary footer_signing_key_metadata
```

使用方调研的目标 footer 需要在相同 field id 上写出另一套非标准语义:

```thrift
8: optional bool encrypted
9: optional string keyname
```

这不能通过现有 `crate::format::FileMetaData` 普通字段赋值实现, 因为 generated serializer 会固定把 field 8 写成 `TType::Struct`, field 9 写成 `TType::String` bytes.

### 1.2 目标

新增 `WriterProperties` 级别的 footer field override 配置:

- 允许调用方指定 footer field id 与对应的 thrift 字段名、类型和值
- 初始版本只允许覆盖 `FileMetaData` 的 optional field 8 和 9
- 写 footer 时, 如果 override 与标准 `FileMetaData` 字段重叠, 使用 override 写出的字段
- 不修改 generated `parquet/src/format.rs`
- 不要求同时配置 `external_encryption`

### 1.3 非目标

- 不开放覆盖 required fields 1-4 (`version`, `schema`, `num_rows`, `row_groups`)
- 初始版本不开放覆盖 field 5-7
- 不保证标准 Parquet reader 能读取覆盖后的非标准 footer
- 不改 Parquet thrift IDL 或重新生成 `format.rs`

## 2. 设计

### 2.1 公开 API

新增类型放在 `parquet/src/file/properties.rs`.

```rust
use std::collections::BTreeMap;

pub type FooterFieldOverrides = BTreeMap<i16, FooterFieldOverride>;

#[derive(Clone, Debug, PartialEq)]
pub struct FooterFieldOverride {
    pub name: String,
    pub value: FooterFieldValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FooterFieldValue {
    Bool(bool),
    String(String),
    Binary(Vec<u8>),
}
```

`WriterProperties` 增加字段:

```rust
pub struct WriterProperties {
    // ... 现有字段保持不变
    footer_field_overrides: Option<FooterFieldOverrides>,
}

impl WriterProperties {
    pub fn footer_field_overrides(&self) -> Option<&FooterFieldOverrides> {
        self.footer_field_overrides.as_ref()
    }
}

pub struct WriterPropertiesBuilder {
    // ... 现有字段保持不变
    footer_field_overrides: Option<FooterFieldOverrides>,
}

impl WriterPropertiesBuilder {
    pub fn with_footer_field_overrides(
        mut self,
        overrides: FooterFieldOverrides,
    ) -> Self {
        self.footer_field_overrides = Some(overrides);
        self
    }
}
```

使用方示例:

```rust
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
            value: FooterFieldValue::String(keyname),
        },
    ),
]);

let props = WriterProperties::builder()
    .with_footer_field_overrides(overrides)
    .build();
```

### 2.2 校验规则

`WriterPropertiesBuilder::build()` 中校验:

1. 只允许 field id `8` 和 `9`.
2. field `8` 只允许 `FooterFieldValue::Bool`.
3. field `9` 只允许 `FooterFieldValue::String` 或 `FooterFieldValue::Binary`.
4. 不允许和标准 `file_encryption_properties` 同时配置.

第 4 条不是因为 footer override 必须依赖 external encryption, 而是因为标准 Parquet plaintext-footer encryption 正好使用 `FileMetaData` field 8/9 写 `EncryptionAlgorithm` 与 footer signing key metadata. 如果同时启用标准 encryption 又覆盖 8/9, 会破坏标准加密 reader 的解密元数据.

不配置 `external_encryption` 时也可以使用 footer override. 这种模式适用于只需要写出非标准 footer 标记, 但 page data 不一定由本 crate 负责外部加密的场景.

### 2.3 写入介入点

footer 写入在 `parquet/src/file/metadata/writer.rs`:

1. `ThriftMetadataWriter::finish()` 组装 `crate::format::FileMetaData`
2. 调用 `MetadataObjectWriter::write_file_metadata(...)`
3. 写出 metadata length 和 magic

现有无加密路径直接调用:

```rust
Self::write_object(file_metadata, sink)
```

新增逻辑:

```rust
fn write_file_metadata(&self, file_metadata: &FileMetaData, sink: impl Write) -> Result<()> {
    match self.footer_field_overrides.as_ref() {
        Some(overrides) => Self::write_file_metadata_with_overrides(
            file_metadata,
            overrides,
            sink,
        ),
        None => Self::write_object(file_metadata, sink),
    }
}
```

`MetadataObjectWriter` 需要能拿到 `WriterProperties.footer_field_overrides`. 可选方式:

- 在创建 `MetadataObjectWriter` 时复制 `Option<FooterFieldOverrides>`
- 或在 `ThriftMetadataWriter` 中保存该配置, 写 footer 时传给 `MetadataObjectWriter`

推荐第一种, 让 footer 序列化决策集中在 `MetadataObjectWriter`.

### 2.4 自定义 `FileMetaData` serializer

新增内部函数:

```rust
fn write_file_metadata_with_overrides(
    file_metadata: &FileMetaData,
    overrides: &FooterFieldOverrides,
    mut sink: impl Write,
) -> Result<()> {
    let mut protocol = TCompactOutputProtocol::new(&mut sink);
    write_file_metadata_fields_1_to_7(file_metadata, &mut protocol)?;
    write_override_fields(overrides, &mut protocol)?;
    Ok(())
}
```

实际实现不复用 `FileMetaData::write_to_out_protocol()`, 而是手动写 `FileMetaData` struct:

1. 写 field 1: `version`
2. 写 field 2: `schema`
3. 写 field 3: `num_rows`
4. 写 field 4: `row_groups`
5. 如存在, 写 field 5: `key_value_metadata`
6. 如存在, 写 field 6: `created_by`
7. 如存在, 写 field 7: `column_orders`
8. 写 override field 8/9
9. 写 stop 和 struct end

因为初始版本只允许覆盖 8/9, field 1-7 的写法可以直接照搬 generated `FileMetaData::write_to_out_protocol()` 中对应字段的序列化逻辑.

override 写入规则:

```rust
match value {
    FooterFieldValue::Bool(v) => {
        protocol.write_field_begin(&TFieldIdentifier::new(&name, TType::Bool, field_id))?;
        protocol.write_bool(*v)?;
        protocol.write_field_end()?;
    }
    FooterFieldValue::String(v) => {
        protocol.write_field_begin(&TFieldIdentifier::new(&name, TType::String, field_id))?;
        protocol.write_string(v)?;
        protocol.write_field_end()?;
    }
    FooterFieldValue::Binary(v) => {
        protocol.write_field_begin(&TFieldIdentifier::new(&name, TType::String, field_id))?;
        protocol.write_bytes(v)?;
        protocol.write_field_end()?;
    }
}
```

### 2.5 与标准 encryption 的关系

footer override 是独立 `WriterProperties` 能力, 不依赖 `external_encryption`.

但它应与标准 `file_encryption_properties` 互斥:

- 标准 encrypted footer (`PARE`) 会在 footer 前写 `FileCryptoMetaData`, 再写 encrypted `FileMetaData`
- 标准 plaintext footer encryption 会使用 `FileMetaData` field 8/9 记录标准 encryption metadata
- footer override 改写 8/9 会破坏这些标准语义

因此推荐校验:

```rust
if self.file_encryption_properties.is_some()
    && self.footer_field_overrides.is_some()
{
    panic!("file_encryption_properties and footer_field_overrides are mutually exclusive");
}
```

与 `external_encryption` 的关系:

- 可以只配置 `external_encryption`
- 可以只配置 `footer_field_overrides`
- 可以同时配置 `external_encryption` 和 `footer_field_overrides`

## 3. 改动文件清单

1. **`parquet/src/file/properties.rs`**
   - 新增 `FooterFieldOverrides`
   - 新增 `FooterFieldOverride`
   - 新增 `FooterFieldValue`
   - `WriterProperties` / `WriterPropertiesBuilder` 增加 `footer_field_overrides`
   - 新增 `with_footer_field_overrides(...)`
   - 在 `build()` 中校验只允许 8/9, 且禁止与 `file_encryption_properties` 同时配置

2. **`parquet/src/file/metadata/writer.rs`**
   - `MetadataObjectWriter` 保存 `footer_field_overrides`
   - 无标准 encryption 时, `write_file_metadata` 检查 overrides
   - 新增 `write_file_metadata_with_overrides`
   - 手动序列化 `FileMetaData` field 1-7, 再写 override field 8/9

3. **无需修改**
   - `parquet/src/format.rs` 不改
   - Parquet thrift IDL 不改
   - 标准 encryption serializer 不改

## 4. 测试策略

1. **单元测试**
   - field 8 bool + field 9 string 能被写入 footer bytes
   - field 8 非 bool 时 `build()` 失败
   - field 9 非 string/binary 时 `build()` 失败
   - 覆盖 1-7 或其他 field id 时 `build()` 失败
   - 同时配置 `file_encryption_properties` 和 `footer_field_overrides` 时失败

2. **序列化测试**
   - 写一个最小文件, 读取尾部 metadata bytes
   - 使用 thrift protocol 验证 field 8 的 `TType::Bool`
   - 验证 field 9 的 `TType::String`
   - 验证 field 1-7 仍可按标准 `FileMetaData` 结构解析

3. **行为测试**
   - 只配置 `footer_field_overrides`, 不配置 `external_encryption`, 文件写出成功
   - 同时配置 `external_encryption` 和 `footer_field_overrides`, 文件写出成功
   - 未配置 overrides 时, footer bytes 与现有路径保持一致

## 5. 风险与限制

| # | 项 | 说明 |
|---|---|---|
| 1 | 非标准 footer | field 8/9 使用了与 Parquet 标准不同的类型和语义. 标准 reader 可能无法读取. |
| 2 | generated serializer 无法复用 | `FileMetaData::write_to_out_protocol()` 会固定写标准 8/9, 因此必须手写 custom serializer. |
| 3 | 与标准 encryption 互斥 | 标准 encryption 依赖 field 8/9 或 encrypted footer metadata. 覆盖后会破坏解密元数据. |
| 4 | 初始只支持 8/9 | 为降低风险, 不开放任意 field override. 未来如确有需要再扩展. |
| 5 | 读路径缺失 | 本方案只写非标准 footer. 读取端需要理解 field 8 bool / field 9 keyname. |

## 6. 决策记录

| 选项 | 决策 | 理由 |
|---|---|---|
| 配置入口 | **`WriterProperties`** | footer 写出是 writer 行为, 与是否启用 external page encryption 无强绑定. |
| override 表达 | **`BTreeMap<i16, FooterFieldOverride>`** | 使用 field id 做 key, 保证写出顺序稳定. |
| value 表达 | **枚举携带具体值** | 仅类型与名称不足以写 footer, 写出时必须有实际值. |
| 支持范围 | **初始只允许 8/9** | 满足当前协议, 避免破坏 required metadata 字段. |
| 是否依赖 external encryption | **不依赖** | 调用方可能只需要非标准 footer 标记, 不一定需要本 crate 执行 page data 加密. |
| 是否允许标准 encryption 同时配置 | **不允许** | 标准 encryption 使用或依赖 field 8/9 语义, 同时配置会生成不可解读的 footer. |
