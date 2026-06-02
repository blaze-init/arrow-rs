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
fn test_footer_field_override_field_9_bytes() {
    // Field 9 (footer_signing_key_metadata) is binary/String type.
    // Using Bytes keeps the type compatible with the standard reader.
    let overrides = BTreeMap::from([(
        9,
        FooterFieldOverride {
            name: "keyname".to_string(),
            value: FooterFieldValue::Bytes(b"my_key".to_vec()),
        },
    )]);
    let buffer = write_file(Some(overrides));

    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");

    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    let file_meta = reader.metadata().file_metadata();
    assert_eq!(file_meta.num_rows(), 3);
}

#[test]
fn test_footer_field_override_field_8_type_change() {
    // Field 8 (encryption_algorithm) is Struct type; Bool changes the type
    // so the standard reader can't parse it. Only verify structure.
    let overrides = BTreeMap::from([(
        8,
        FooterFieldOverride {
            name: "encrypted".to_string(),
            value: FooterFieldValue::Bool(true),
        },
    )]);
    let buffer = write_file(Some(overrides));

    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");
    let metadata_len_pos = buffer.len() - 8;
    let metadata_len = u32::from_le_bytes(
        buffer[metadata_len_pos..metadata_len_pos + 4]
            .try_into()
            .unwrap(),
    );
    assert!(metadata_len > 0);
    assert!(metadata_len as usize <= metadata_len_pos);
}

#[test]
fn test_footer_field_override_field_8_9_type_change() {
    // Both fields use non-standard types
    let overrides = BTreeMap::from([
        (
            8,
            FooterFieldOverride {
                name: "encrypted".to_string(),
                value: FooterFieldValue::Int32(42),
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

    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");
    let metadata_len_pos = buffer.len() - 8;
    let metadata_len = u32::from_le_bytes(
        buffer[metadata_len_pos..metadata_len_pos + 4]
            .try_into()
            .unwrap(),
    );
    assert!(metadata_len > 0);
    assert!(metadata_len as usize <= metadata_len_pos);
}

#[test]
fn test_footer_no_overrides_unchanged() {
    // Without overrides, footer bytes should be identical across runs
    let buffer_default = write_file(None);
    let buffer_default2 = write_file(None);
    assert_eq!(buffer_default, buffer_default2);
}
