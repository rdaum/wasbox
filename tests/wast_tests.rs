// Copyright (C) 2024 Ryan Daum <ryan.daum@gmail.com>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

#[cfg(test)]
mod tests {
    use std::path::Path;
    use wasbox::{mk_instance, DecodeError, Execution, LoaderError, Module};
    use wast::core::{NanPattern, WastArgCore, WastRetCore};
    use wast::lexer::Lexer;
    use wast::{
        parser, QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
    };

    enum DecodeResult {
        Success(()),
        Failure(LoaderError),
    }

    fn module_decode(path: &Path) -> Vec<(usize, Option<String>, DecodeResult, Vec<u8>)> {
        let file = std::fs::File::open(path).unwrap();
        let input = std::io::read_to_string(file).unwrap();

        let mut lexer = Lexer::new(&input);
        lexer.allow_confusing_unicode(path.ends_with("names.wast"));
        let pb = wast::parser::ParseBuffer::new_with_lexer(lexer).unwrap();
        let ast = parser::parse::<Wast>(&pb)
            .unwrap_or_else(|_| panic!("Failed to parse WAST file {:?}", path));

        let mut test_directives = vec![];
        let mut found_modules = vec![];
        for (n, directive) in ast.directives.into_iter().enumerate() {
            if let WastDirective::Module(mut module) = directive {
                let (is_module, name) = match &module {
                    QuoteWat::Wat(Wat::Module(m)) => (true, m.id),
                    QuoteWat::QuoteModule(..) => (true, None),
                    QuoteWat::Wat(Wat::Component(m)) => (false, m.id),
                    QuoteWat::QuoteComponent(..) => (false, None),
                };
                let name = name.map(|n| n.name().to_string());

                if is_module {
                    found_modules.push((n, name, module.encode().unwrap()));
                }
            } else {
                test_directives.push(directive);
            }
        }

        let mut decode_results = vec![];
        for (n, name, module_bytes) in found_modules {
            // Load these module bytes as a wasbox module.
            let binary = module_bytes.clone();
            match Module::load(&module_bytes) {
                Ok(_m) => {
                    decode_results.push((n, name, DecodeResult::Success(()), binary));
                }
                Err(e) => {
                    decode_results.push((n, name, DecodeResult::Failure(e), binary));
                }
            };
        }
        decode_results
    }

    /// Scan the entire testsuite directory and attempt to decode all the tests, then assert
    /// no failures, and report the failures.
    /// Unsupported features are expected to fail, and are ignored.
    #[test]
    fn test_scan_decode_all_tests() {
        let dir = std::fs::read_dir("tests/testsuite").unwrap();
        let mut failures = vec![];
        let mut attempts = 0;
        for entry in dir {
            attempts += 1;
            let entry = entry.unwrap();
            let path = entry.path();
            if !path.extension().map_or(false, |ext| ext == "wast") {
                continue;
            }
            let results = module_decode(&path);
            for (n, name, decode_result, bin) in results {
                match decode_result {
                    DecodeResult::Success(_)
                    | DecodeResult::Failure(LoaderError::DecoderError(
                        DecodeError::UnsupportedType(_, _),
                    ))
                    | DecodeResult::Failure(LoaderError::DecoderError(
                        DecodeError::UnimplementedOpcode(_, _),
                    )) => {}
                    DecodeResult::Failure(e) => {
                        failures.push((n, path.clone(), name, e, bin));
                    }
                }
            }
        }
        if !failures.is_empty() {
            eprintln!("{} failures in {} files", failures.len(), attempts);
            for (n, path, name, e, binary) in failures {
                eprintln!("  {path:?}/{name:?} #{n} => {e:?}");
                // Print module binary as hex
                for (i, chunk) in binary.chunks(16).enumerate() {
                    eprintln!("{:x}\t\t{:02x?}", i * 16, chunk);
                }
            }
            panic!("failures present");
        }
    }

    fn convert_value(v: &WastArg) -> wasbox::Value {
        match v {
            WastArg::Core(WastArgCore::I32(i)) => wasbox::Value::I32(*i),
            WastArg::Core(WastArgCore::I64(i)) => wasbox::Value::I64(*i),
            WastArg::Core(WastArgCore::F32(f)) => wasbox::Value::F32(f32::from_bits(f.bits)),
            WastArg::Core(WastArgCore::F64(f)) => wasbox::Value::F64(f64::from_bits(f.bits)),

            _ => panic!("Unsupported arg type"),
        }
    }

    fn convert_ret(v: &WastRet) -> wasbox::Value {
        match v {
            WastRet::Core(WastRetCore::I32(i)) => wasbox::Value::I32(*i),
            WastRet::Core(WastRetCore::I64(i)) => wasbox::Value::I64(*i),
            WastRet::Core(WastRetCore::F32(NanPattern::Value(f))) => {
                wasbox::Value::F32(f32::from_bits(f.bits))
            }
            WastRet::Core(WastRetCore::F32(NanPattern::ArithmeticNan)) => {
                wasbox::Value::F32(f32::NAN)
            }
            WastRet::Core(WastRetCore::F32(NanPattern::CanonicalNan)) => {
                wasbox::Value::F32(f32::NAN)
            }
            WastRet::Core(WastRetCore::F64(NanPattern::Value(f))) => {
                wasbox::Value::F64(f64::from_bits(f.bits))
            }
            WastRet::Core(WastRetCore::F64(NanPattern::ArithmeticNan)) => {
                wasbox::Value::F64(f64::NAN)
            }
            WastRet::Core(WastRetCore::F64(NanPattern::CanonicalNan)) => {
                wasbox::Value::F64(f64::NAN)
            }
            _ => panic!("Unsupported ret type"),
        }
    }

    fn perform_execution_test(path: &Path) {
        let file = std::fs::File::open(path).unwrap();
        let input = std::io::read_to_string(file).unwrap();

        let lexer = Lexer::new(&input);
        let pb = wast::parser::ParseBuffer::new_with_lexer(lexer).unwrap();
        let ast = parser::parse::<Wast>(&pb)
            .unwrap_or_else(|_| panic!("Failed to parse WAST file {:?}", path));

        let mut execution = None;
        for directive in ast.directives {
            match directive {
                WastDirective::Module(mut module) => {
                    let m = Module::load(&module.encode().unwrap()).unwrap();
                    let i = mk_instance(m).unwrap();
                    let memory = i.memories[0].clone();
                    execution = Some(Execution::new(i, memory));
                }
                WastDirective::AssertReturn { exec, results, .. } => match exec {
                    WastExecute::Invoke(WastInvoke {
                        span: _,
                        module: _,
                        name,
                        args,
                    }) => {
                        let execution = execution.as_mut().unwrap();
                        let funcidx = execution
                            .instance()
                            .find_funcidx(name)
                            .unwrap_or_else(|| panic!("Function not found: {:?}", name));

                        let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                        execution.prepare(funcidx, &arg_set).unwrap();
                        execution.run().unwrap();
                        let result = execution.result().unwrap();

                        let expected_results: Vec<_> = results.iter().map(convert_ret).collect();
                        for (i, (expected, actual)) in
                            expected_results.iter().zip(result.iter()).enumerate()
                        {
                            assert!(
                                expected.eq_w_nan(actual),
                                "Mismatch at index {}: expected {:?}, got {:?}",
                                i,
                                expected,
                                actual
                            );
                        }
                    }
                    _ => panic!("Unsupported exec directive"),
                },
                _ => {}
            }
        }
    }

    #[test]
    fn address_test() {
        let path = Path::new("tests/testsuite/address.wast");
        perform_execution_test(path);
    }
}
