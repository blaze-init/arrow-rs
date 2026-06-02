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

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use bytes::Bytes;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::errors::Result;
use parquet::file::properties::{ExternalEncryptFn, ExternalEncryption, WriterProperties};
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
