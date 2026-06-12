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

use crate::file::properties::{
    FooterFieldReadOverrides, FooterFieldValue, FooterFieldValueType, FooterFieldValues,
};
use thrift::protocol::{
    TFieldIdentifier, TInputProtocol, TListIdentifier, TMapIdentifier, TMessageIdentifier,
    TOutputProtocol, TSetIdentifier, TStructIdentifier, TType,
};

pub(super) fn write_footer_field_value<T: TOutputProtocol>(
    protocol: &mut T,
    field_id: i16,
    field_name: &str,
    value: &FooterFieldValue,
) -> thrift::Result<()> {
    match value {
        FooterFieldValue::Bool(v) => {
            protocol.write_field_begin(&TFieldIdentifier::new(
                field_name,
                TType::Bool,
                field_id,
            ))?;
            protocol.write_bool(*v)?;
        }
        FooterFieldValue::String(v) => {
            protocol.write_field_begin(&TFieldIdentifier::new(
                field_name,
                TType::String,
                field_id,
            ))?;
            protocol.write_string(v)?;
        }
        FooterFieldValue::Int32(v) => {
            protocol.write_field_begin(&TFieldIdentifier::new(field_name, TType::I32, field_id))?;
            protocol.write_i32(*v)?;
        }
        FooterFieldValue::Int64(v) => {
            protocol.write_field_begin(&TFieldIdentifier::new(field_name, TType::I64, field_id))?;
            protocol.write_i64(*v)?;
        }
        FooterFieldValue::Bytes(v) => {
            protocol.write_field_begin(&TFieldIdentifier::new(
                field_name,
                TType::String,
                field_id,
            ))?;
            protocol.write_bytes(v)?;
        }
    }
    protocol.write_field_end()
}

fn read_footer_field_value<T: TInputProtocol>(
    protocol: &mut T,
    field_id: i16,
    actual_type: TType,
    expected_type: FooterFieldValueType,
) -> thrift::Result<FooterFieldValue> {
    let expected_ttype = match expected_type {
        FooterFieldValueType::Bool => TType::Bool,
        FooterFieldValueType::String | FooterFieldValueType::Bytes => TType::String,
        FooterFieldValueType::Int32 => TType::I32,
        FooterFieldValueType::Int64 => TType::I64,
    };
    if actual_type != expected_ttype {
        let type_name = match expected_type {
            FooterFieldValueType::Bool => "bool",
            FooterFieldValueType::String => "string",
            FooterFieldValueType::Int32 => "i32",
            FooterFieldValueType::Int64 => "i64",
            FooterFieldValueType::Bytes => "bytes",
        };
        return Err(thrift::Error::Protocol(thrift::ProtocolError {
            kind: thrift::ProtocolErrorKind::InvalidData,
            message: format!(
                "footer field override {} expected {} but found {:?}",
                field_id, type_name, actual_type
            ),
        }));
    }

    match expected_type {
        FooterFieldValueType::Bool => protocol.read_bool().map(FooterFieldValue::Bool),
        FooterFieldValueType::String => protocol.read_string().map(FooterFieldValue::String),
        FooterFieldValueType::Int32 => protocol.read_i32().map(FooterFieldValue::Int32),
        FooterFieldValueType::Int64 => protocol.read_i64().map(FooterFieldValue::Int64),
        FooterFieldValueType::Bytes => protocol.read_bytes().map(FooterFieldValue::Bytes),
    }
}

pub(super) struct OverrideInputProtocol<'a, T: TInputProtocol> {
    inner: &'a mut T,
    overrides: &'a FooterFieldReadOverrides,
    values: FooterFieldValues,
    structs: u32,
}

impl<'a, T: TInputProtocol> OverrideInputProtocol<'a, T> {
    pub(super) fn new(inner: &'a mut T, overrides: &'a FooterFieldReadOverrides) -> Self {
        Self {
            inner,
            overrides,
            values: FooterFieldValues::default(),
            structs: 0,
        }
    }

    pub(super) fn into_values(self) -> FooterFieldValues {
        self.values
    }
}

impl<T: TInputProtocol> TInputProtocol for OverrideInputProtocol<'_, T> {
    fn read_message_begin(&mut self) -> thrift::Result<TMessageIdentifier> {
        self.inner.read_message_begin()
    }

    fn read_message_end(&mut self) -> thrift::Result<()> {
        self.inner.read_message_end()
    }

    fn read_struct_begin(&mut self) -> thrift::Result<Option<TStructIdentifier>> {
        self.structs += 1;
        self.inner.read_struct_begin()
    }

    fn read_struct_end(&mut self) -> thrift::Result<()> {
        self.inner.read_struct_end()?;
        self.structs = self.structs.saturating_sub(1);
        Ok(())
    }

    fn read_field_begin(&mut self) -> thrift::Result<TFieldIdentifier> {
        loop {
            let field = self.inner.read_field_begin()?;
            if field.field_type == TType::Stop {
                return Ok(field);
            }

            let Some(field_id) = field.id else {
                return Ok(field);
            };

            if self.structs != 1 {
                return Ok(field);
            }

            let Some(expected_type) = self.overrides.get(&field_id).copied() else {
                return Ok(field);
            };

            let value =
                read_footer_field_value(self.inner, field_id, field.field_type, expected_type)?;
            self.inner.read_field_end()?;
            self.values.insert(field_id, value);
        }
    }

    fn read_field_end(&mut self) -> thrift::Result<()> {
        self.inner.read_field_end()
    }

    fn read_bool(&mut self) -> thrift::Result<bool> {
        self.inner.read_bool()
    }

    fn read_bytes(&mut self) -> thrift::Result<Vec<u8>> {
        self.inner.read_bytes()
    }

    fn read_i8(&mut self) -> thrift::Result<i8> {
        self.inner.read_i8()
    }

    fn read_i16(&mut self) -> thrift::Result<i16> {
        self.inner.read_i16()
    }

    fn read_i32(&mut self) -> thrift::Result<i32> {
        self.inner.read_i32()
    }

    fn read_i64(&mut self) -> thrift::Result<i64> {
        self.inner.read_i64()
    }

    fn read_double(&mut self) -> thrift::Result<f64> {
        self.inner.read_double()
    }

    fn read_string(&mut self) -> thrift::Result<String> {
        self.inner.read_string()
    }

    fn read_list_begin(&mut self) -> thrift::Result<TListIdentifier> {
        self.inner.read_list_begin()
    }

    fn read_list_end(&mut self) -> thrift::Result<()> {
        self.inner.read_list_end()
    }

    fn read_set_begin(&mut self) -> thrift::Result<TSetIdentifier> {
        self.inner.read_set_begin()
    }

    fn read_set_end(&mut self) -> thrift::Result<()> {
        self.inner.read_set_end()
    }

    fn read_map_begin(&mut self) -> thrift::Result<TMapIdentifier> {
        self.inner.read_map_begin()
    }

    fn read_map_end(&mut self) -> thrift::Result<()> {
        self.inner.read_map_end()
    }

    fn read_byte(&mut self) -> thrift::Result<u8> {
        self.inner.read_byte()
    }
}
