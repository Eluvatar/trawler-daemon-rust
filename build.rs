// Copyright 2015 Eluvatar
//
// This file is part of Trawler.
//
// Trawler is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Trawler is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Trawler.  If not, see <http://www.gnu.org/licenses/>.

extern crate filetime;
use std::fs;
use filetime::FileTime;
use std::process::Command;

fn build(in_path: &str, out_path: &str, method: fn() -> () ) {
    let inmeta = fs::metadata(in_path).unwrap();
    let intime = FileTime::from_last_modification_time(&inmeta);

    let outtime = match fs::metadata(out_path) {
        Ok(outmeta) => FileTime::from_last_modification_time(&outmeta),
        Err(err) => match err.kind() {
            std::io::ErrorKind::NotFound => FileTime::zero(),
            _ => panic!("checking modification time of {} failed: error {:?}", out_path, err),
        }
    };

    if intime > outtime  {
        method();
    }
}

fn protoc() {
    let status = Command::new("protoc")
                         .arg("--proto_path=protocol")
                         .arg("--rust_out=src")
                         .arg("protocol/trawler.proto")
                         .status()
                         .unwrap_or_else(|e| panic!("failed to execute protoc: {:?}",e));
    if ! status.success() {
        panic!("protoc failed! status: {:?}", status.code())
    }
}

fn main() {
    build("protocol/trawler.proto","src/trawler.rs",protoc);
}
