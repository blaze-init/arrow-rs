#!/usr/bin/env bash

# Licensed to the Apache Software Foundation (ASF) under one
# or more contributor license agreements.  See the NOTICE file
# distributed with this work for additional information
# regarding copyright ownership.  The ASF licenses this file
# to you under the Apache License, Version 2.0 (the
# "License"); you may not use this file except in compliance
# with the License.  You may obtain a copy of the License at
#
#   http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing,
# software distributed under the License is distributed on an
# "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
# KIND, either express or implied.  See the License for the
# specific language governing permissions and limitations
# under the License.

REVISION=46cc3a0647d301bb9579ca8dd2cc356caf2a72d2

SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"

_sed() {
  if [ `uname` == Darwin ]; then
    gsed "$@"
  else
    sed "$@"
  fi
}

  thrift --gen rs parquet_kwai.thrift && \
  echo "Removing TProcessor" && \
  _sed -i '/use thrift::server::TProcessor;/d' parquet_kwai.rs && \
  echo "Replacing TSerializable" && \
  _sed -i 's/impl TSerializable for/impl crate::thrift::TSerializable for/g' parquet_kwai.rs && \
  echo "Rewriting write_to_out_protocol" && \
  _sed -i 's/fn write_to_out_protocol(&self, o_prot: &mut dyn TOutputProtocol)/fn write_to_out_protocol<T: TOutputProtocol>(\&self, o_prot: \&mut T)/g' parquet_kwai.rs && \
  echo "Rewriting read_from_in_protocol" && \
  _sed -i 's/fn read_from_in_protocol(i_prot: &mut dyn TInputProtocol)/fn read_from_in_protocol<T: TInputProtocol>(i_prot: \&mut T)/g' parquet_kwai.rs && \
  mv parquet_kwai.rs src/format.rs
