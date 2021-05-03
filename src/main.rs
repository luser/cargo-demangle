// Copyright 2016 Mozilla
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_use]
extern crate clap;
#[macro_use]
extern crate lazy_static;
extern crate regex;
extern crate rustc_demangle;

use regex::{Regex, Captures};

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write, stdin, stdout, stderr};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::process::exit;

mod tests;

lazy_static! {
    // NOTE: Use [[:alnum::]] instead of \w to only match ASCII word characters, not unicode
    static ref MANGLED_NAME_PATTERN: Regex = Regex::new(r"_(ZN|R)[\$\._[:alnum:]]*").unwrap();
}

fn demangle(symbol: &str, include_hash: bool) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let demangled = rustc_demangle::demangle(symbol);

    let result = if include_hash {
        write!(out, "{}", demangled)
    } else {
        // Use alternate formatting to exclude the hash from the result
        write!(out, "{:#}", demangled)
    };

    if result.is_err() {
        out.clear();
        out.push_str(symbol);
    }

    out
}

#[inline] // Except for the nested functions (which don't count), this is a very small function
pub fn demangle_line(line: &str, include_hash: bool) -> Cow<str> {
    MANGLED_NAME_PATTERN.replace_all(line, |captures: &Captures| {
        demangle(&captures[0], include_hash)
    })
}

fn demangle_stream<R: BufRead, W: Write>(input: &mut R, output: &mut W, include_hash: bool) -> io::Result<()> {
    // NOTE: this is actually more efficient than lines(), since it re-uses the buffer
    let mut buf = String::new();
    while input.read_line(&mut buf)? > 0 {
        {
            // NOTE: This includes the line-ending, and leaves it untouched
            let demangled_line = demangle_line(&buf, include_hash);
            if cfg!(debug_assertions) && buf.ends_with('\n') {
                let line_ending = if buf.ends_with("\r\n") { "\r\n" } else { "\n" };
                debug_assert!(demangled_line.ends_with(line_ending), "Demangled line has incorrect line ending");
            }
            output.write_all(demangled_line.as_bytes())?;
        }
        buf.clear(); // Reset the buffer's position, without freeing it's underlying memory
    }
    Ok(()) // Successfully hit EOF
}

enum InputType {
    Stdin,
    File(PathBuf)
}

impl InputType {
    fn demangle(&self, output: OutputType, include_hash: bool) -> io::Result<()> {
        // NOTE: This has to be seperated into two functions for generics
        match *self {
            InputType::Stdin => {
                let stdin = stdin();
                let mut lock = stdin.lock();
                output.write_demangled(&mut lock, include_hash)
            },
            InputType::File(ref path) => output.write_demangled(&mut BufReader::new(File::open(path)?), include_hash)
        }
    }
    fn validate(file: String) -> Result<(), String> {
        file.parse::<InputType>().map(|_| ())
    }
}
impl FromStr for InputType {
    type Err = String;
    fn from_str(file: &str) -> Result<InputType, String> {
        if file == "-" {
            Ok(InputType::Stdin)
        } else {
            let path = Path::new(&file);
            if !path.is_file() {
                if !path.exists() {
                    Err(format!("{} doesn't exist", file))
                } else {
                    Err(format!("{} isn't a file", file))
                }
            } else {
                Ok(InputType::File(PathBuf::from(path)))
            }
        }
    }
}

enum OutputType {
    Stdout,
    File(PathBuf)
}

impl OutputType {
    #[inline] // It's only used twice
    fn write_demangled<I: io::BufRead>(&self, input: &mut I, include_hash: bool) -> io::Result<()> {
        match *self {
            OutputType::Stdout => {
                let stdout = stdout();
                let mut lock = stdout.lock();
                demangle_stream(input, &mut lock, include_hash)
            },
            OutputType::File(ref path) => {
                let file = File::create(path)?;
                let mut buf = BufWriter::new(&file);
                demangle_stream(input, &mut buf, include_hash)
            }
        }
    }
    fn write_demangled_names<S: AsRef<str>>(&self, names: &[S], include_hash: bool) -> io::Result<()> {
        #[inline] // It's only used twice ;)
        fn demangle_names_to<S: AsRef<str>, O: io::Write>(names: &[S], output: &mut O, include_hash: bool) -> io::Result<()> {
            for name in names {
                writeln!(output, "{}", demangle(name.as_ref(), include_hash))?;
            }
            Ok(())
        }
        match *self {
            OutputType::Stdout => {
                let stdout = stdout();
                let mut lock = stdout.lock();
                demangle_names_to(names, &mut lock, include_hash)
            },
            OutputType::File(ref path) => {
                let file = File::create(path)?;
                let mut buf = BufWriter::new(&file);
                demangle_names_to(names, &mut buf, include_hash)
            }
        }
    }
    fn validate(file: String) -> Result<(), String> {
        file.parse::<OutputType>().map(|_| ())
    }
}
impl FromStr for OutputType {
    type Err = String;
    fn from_str(file: &str) -> Result<OutputType, String> {
        if file == "-" {
            Ok(OutputType::Stdout)
        } else {
            let path = Path::new(&file);
            if path.exists() {
                Err(format!("{} already exists", file))
            } else {
                Ok(OutputType::File(PathBuf::from(path)))
            }
        }
    }
}

fn main() {
    let args: clap::ArgMatches = clap_app!(rust_demangle =>
        (version: crate_version!())
        (author: crate_authors!())
        (about: "Demangles names generated by the rust compiler")
        (@arg INCLUDE_HASH: --include-hash --hash "Include the hashes in the demangled names")
        (@arg INPUT: --input -i [FILE] default_value("-") {InputType::validate} "The input file to replace the mangled names in, or '-' for stdin")
        (@arg OUTPUT: --output -o [FILE] default_value("-") {OutputType::validate} "The output file to emit the demangled names to, or '-' for stdout")
        (@arg NAMES: ... [NAME] conflicts_with[INPUT] "The list of names to demangle")
    ).get_matches();
    let include_hash = args.is_present("INCLUDE_HASH");
    let output = value_t!(args, "OUTPUT", OutputType).unwrap();
    if let Some(names) = args.values_of("NAMES") {
        output.write_demangled_names(&names.collect::<Vec<_>>(), include_hash).unwrap_or_else(|e| {
            writeln!(stderr(), "Unable to demangle names: {}", e).unwrap();
            exit(1);
        })
    } else {
        let input = value_t!(args, "INPUT", InputType).unwrap();
        input.demangle(output, include_hash).unwrap_or_else(|e| {
            writeln!(stderr(), "Unable to demangle input: {}", e).unwrap();
            exit(1);
        })
    }
}
