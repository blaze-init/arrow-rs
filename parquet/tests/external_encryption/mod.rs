// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_array::Array;
use bytes::Bytes;
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::errors::{ParquetError, Result};
use parquet::file::properties::{
    ExternalDecryptFn, ExternalDecryption, ExternalEncryptFn, ExternalEncryption,
    WriterProperties,
};
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    let mut writer = ArrowWriter::try_new(&mut buf, schema, props).unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
    buf
}

#[test]
fn test_par1_magic() {
    let buffer = write_batch(
        &make_batch(),
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );
    assert_eq!(&buffer[0..4], b"PAR1");
    assert_eq!(&buffer[buffer.len() - 4..], b"PAR1");
}

#[test]
fn test_plaintext_metadata() {
    let buffer = write_batch(
        &make_batch(),
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );
    let reader = SerializedFileReader::new(Bytes::from(buffer)).unwrap();
    let file_meta = reader.metadata().file_metadata();
    assert_eq!(file_meta.num_rows(), 3);
}

fn xor_decrypt_fn() -> ExternalDecryptFn {
    // XOR 0xFF is its own inverse
    Arc::new(xor_encrypt)
}

#[test]
fn test_external_encryption_roundtrip() {
    let batch = make_batch();
    let encrypted_buf = write_batch(
        &batch,
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );

    let options = ArrowReaderOptions::new()
        .with_external_encrypted(true)
        .with_external_decryption(ExternalDecryption::new(xor_decrypt_fn()));

    let reader = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::from(encrypted_buf),
        options,
    )
    .unwrap()
    .build()
    .unwrap();

    let read_batches: Vec<_> = reader.map(|r| r.unwrap()).collect();
    assert_eq!(read_batches.len(), 1);
    assert_eq!(read_batches[0], batch);
}

#[test]
fn test_external_encrypted_missing_decryptor() {
    let batch = make_batch();
    let encrypted_buf = write_batch(
        &batch,
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );

    let options = ArrowReaderOptions::new().with_external_encrypted(true);

    let result = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::from(encrypted_buf),
        options,
    )
    .unwrap()
    .build()
    .unwrap()
    .next();

    match result {
        Some(Err(e)) => {
            let msg = e.to_string().to_lowercase();
            assert!(
                msg.contains("external") && msg.contains("decrypt"),
                "error should mention external decryption: {e}"
            );
        }
        other => panic!("expected external decryption error, got {other:?}"),
    }
}

#[test]
fn test_external_encrypted_flag_false_no_decrypt() {
    let batch = make_batch();
    let plain_buf = write_batch(&batch, None);

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fn = Arc::clone(&calls);
    let decryptor: ExternalDecryptFn = Arc::new(move |data: &[u8]| {
        calls_for_fn.fetch_add(1, Ordering::SeqCst);
        Ok(data.to_vec())
    });

    // external_decryption is configured but external_encrypted defaults to false,
    // reader must never call the decryptor.
    let options =
        ArrowReaderOptions::new().with_external_decryption(ExternalDecryption::new(decryptor));
    let reader = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::from(plain_buf),
        options,
    )
    .unwrap()
    .build()
    .unwrap();

    let read_batches: Vec<_> = reader.map(|r| r.unwrap()).collect();
    assert_eq!(read_batches.len(), 1);
    assert_eq!(read_batches[0], batch);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn test_external_encryption_dictionary_pages_not_decrypted() {
    // String columns tend to trigger dictionary encoding
    let col = Arc::new(StringArray::from_iter_values(["a", "b", "a", "b", "c"])) as ArrayRef;
    let batch = RecordBatch::try_from_iter([("s", col)]).unwrap();
    let schema = batch.schema();

    let props = WriterProperties::builder()
        .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
        .build();

    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fn = Arc::clone(&calls);
    let decryptor: ExternalDecryptFn = Arc::new(move |data: &[u8]| {
        calls_for_fn.fetch_add(1, Ordering::SeqCst);
        xor_encrypt(data)
    });

    let options = ArrowReaderOptions::new()
        .with_external_encrypted(true)
        .with_external_decryption(ExternalDecryption::new(decryptor));

    let reader = ParquetRecordBatchReaderBuilder::try_new_with_options(Bytes::from(buf), options)
        .unwrap()
        .build()
        .unwrap();

    let read_batches: Vec<_> = reader.map(|r| r.unwrap()).collect();
    assert_eq!(read_batches.len(), 1);

    let read_strings = read_batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let expected = vec!["a", "b", "a", "b", "c"];
    let actual: Vec<&str> = (0..read_strings.len())
        .map(|i| read_strings.value(i))
        .collect();
    assert_eq!(actual, expected);

    // This input should produce 1 dictionary page and 1 data page.
    // External decrypt should only act on the data page, so call count must be 1.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_external_encryption_with_page_index() {
    let batch = make_batch();
    let encrypted_buf = write_batch(
        &batch,
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );

    // Enable page index — this makes SerializedPageReader use the Pages state branch
    let options = ArrowReaderOptions::new()
        .with_page_index(true)
        .with_external_encrypted(true)
        .with_external_decryption(ExternalDecryption::new(xor_decrypt_fn()));

    let reader = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::from(encrypted_buf),
        options,
    )
    .unwrap()
    .build()
    .unwrap();

    let read_batches: Vec<_> = reader.map(|r| r.unwrap()).collect();
    assert_eq!(read_batches.len(), 1);
    assert_eq!(read_batches[0], batch);
}

#[test]
fn test_external_decrypt_closure_error() {
    let batch = make_batch();
    let encrypted_buf = write_batch(
        &batch,
        Some(
            WriterProperties::builder()
                .with_external_encryption(ExternalEncryption::new(encrypt_fn()))
                .build(),
        ),
    );

    let decryptor: ExternalDecryptFn = Arc::new(move |_data: &[u8]| {
        Err(ParquetError::General("keycenter: invalid token".into()))
    });

    let options = ArrowReaderOptions::new()
        .with_external_encrypted(true)
        .with_external_decryption(ExternalDecryption::new(decryptor));

    let result = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::from(encrypted_buf),
        options,
    )
    .unwrap()
    .build()
    .unwrap()
    .next();

    match result {
        Some(Err(e)) => {
            assert!(
                e.to_string().contains("invalid token"),
                "error should propagate closure error message: {e}"
            );
        }
        other => panic!("expected decrypt error, got {other:?}"),
    }
}
