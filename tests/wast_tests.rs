// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
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
    use std::fmt::{Debug, Formatter};
    use std::path::Path;
    use wasbox::{
        mk_instance, DecodeError, Execution, LinkError, LoaderError, Module, VectorMemory,
    };
    use wast::core::{NanPattern, WastArgCore, WastRetCore};
    use wast::lexer::Lexer;
    use wast::{
        parser, QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
    };

    macro_rules! wast_test {
        ($test_name:ident, $wast_file:literal) => {
            #[test]
            fn $test_name() {
                let path = Path::new(concat!("tests/testsuite/", $wast_file));
                perform_wast(path);
            }
        };
    }

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
            .unwrap_or_else(|_| panic!("Failed to parse WAST file {path:?}"));

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
            if path.extension().is_none_or(|ext| ext != "wast") {
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
                        DecodeError::MalformedMemory(_),
                    )) => {}
                    DecodeResult::Failure(LoaderError::DecoderError(
                        DecodeError::UnimplementedOpcode(_, _),
                    )) => {}
                    DecodeResult::Failure(e) => {
                        failures.push((n, path.clone(), name, e, bin));
                    }
                }
            }
        }
        if !failures.is_empty() {
            let mut failure_summary =
                format!("{} failures in {} attempts:\n", failures.len(), attempts);
            for (n, path, name, e, _binary) in failures {
                failure_summary.push_str(&format!("  {path:?}/{name:?} #{n} => {e:?}\n"));
            }
            panic!("{}", failure_summary);
        }
    }

    fn convert_value(v: &WastArg) -> wasbox::Value {
        match v {
            WastArg::Core(WastArgCore::I32(i)) => wasbox::Value::I32(*i),
            WastArg::Core(WastArgCore::I64(i)) => wasbox::Value::I64(*i),
            WastArg::Core(WastArgCore::F32(f)) => wasbox::Value::F32(f32::from_bits(f.bits)),
            WastArg::Core(WastArgCore::F64(f)) => wasbox::Value::F64(f64::from_bits(f.bits)),
            WastArg::Core(WastArgCore::RefExtern(r)) => wasbox::Value::ExternRef(Some(*r)),
            WastArg::Core(WastArgCore::RefNull(ref_type)) => {
                // For now, just determine type based on context or default to ExternRef
                // TODO: Check the actual heap type when we have better type info
                match format!("{ref_type:?}").as_str() {
                    s if s.contains("Func") => wasbox::Value::FuncRef(None),
                    _ => wasbox::Value::ExternRef(None),
                }
            }

            _ => panic!("Unsupported arg type: {v:?}"),
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
            WastRet::Core(WastRetCore::RefExtern(r)) => wasbox::Value::ExternRef(*r),
            WastRet::Core(WastRetCore::RefNull(ref_type)) => {
                // Determine type based on context
                match format!("{ref_type:?}").as_str() {
                    s if s.contains("Func") => wasbox::Value::FuncRef(None),
                    _ => wasbox::Value::ExternRef(None),
                }
            }
            _ => panic!("Unsupported ret type"),
        }
    }

    enum TestModule {
        None,
        Loaded(Box<Execution<VectorMemory>>),
        LoadFailed(LoaderError),
        LinkFailed(LinkError),
    }

    impl Debug for TestModule {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                TestModule::None => write!(f, "None"),
                TestModule::Loaded(_) => write!(f, "Loaded"),
                TestModule::LoadFailed(e) => write!(f, "LoadFailed({e:?})"),
                TestModule::LinkFailed(e) => write!(f, "LinkFailed({e:?})"),
            }
        }
    }

    impl TestModule {
        fn load(binary: &[u8]) -> Self {
            let m = Module::load(binary);
            match m {
                Ok(m) => match mk_instance(m) {
                    Ok(i) => {
                        // Use first memory if available, otherwise create a dummy memory
                        let memory = if !i.memories.is_empty() {
                            i.memories[0].clone()
                        } else {
                            VectorMemory::new(0, None)
                        };
                        TestModule::Loaded(Box::new(Execution::new(i, memory)))
                    }
                    Err(e) => TestModule::LinkFailed(e),
                },
                Err(e) => TestModule::LoadFailed(e),
            }
        }
    }

    fn perform_wast(path: &Path) {
        let file = std::fs::File::open(path).unwrap();
        let input = std::io::read_to_string(file).unwrap();

        let lexer = Lexer::new(&input);
        let pb = wast::parser::ParseBuffer::new_with_lexer(lexer).unwrap();
        let ast = parser::parse::<Wast>(&pb)
            .unwrap_or_else(|_| panic!("Failed to parse WAST file {path:?}"));

        let mut execution = TestModule::None;
        for (directive_num, directive) in ast.directives.into_iter().enumerate() {
            let directive_span = directive.span();
            let linecol = directive_span.linecol_in(&input);

            match directive {
                WastDirective::Module(mut module) => {
                    let encoded = module.encode().unwrap();
                    let m = Module::load(&encoded);
                    execution = match m {
                        Ok(m) => match mk_instance(m) {
                            Ok(i) => {
                                // Use first memory if available, otherwise create a dummy memory
                                let memory = if !i.memories.is_empty() {
                                    i.memories[0].clone()
                                } else {
                                    VectorMemory::new(0, None)
                                };
                                TestModule::Loaded(Box::new(Execution::new(i, memory)))
                            }
                            Err(e) => {
                                eprintln!("Link failed at directive #{directive_num} @ {linecol:?}: {e:?}");
                                TestModule::LinkFailed(e)
                            }
                        },
                        Err(e) => {
                            eprintln!(
                                "Load failed at directive #{directive_num} @ {linecol:?}: {e:?}"
                            );
                            TestModule::LoadFailed(e)
                        }
                    };
                }
                WastDirective::AssertReturn { exec, results, .. } => match exec {
                    // Invoke runs executions on the last loaded module.
                    WastExecute::Invoke(WastInvoke {
                        span: _,
                        module: _,
                        name,
                        args,
                    }) => {
                        let TestModule::Loaded(ref mut execution) = execution else {
                            panic!("Expected a loaded module for invocation @ {linecol:?}");
                        };
                        let funcidx = execution
                            .instance()
                            .find_funcidx(name)
                            .unwrap_or_else(|| panic!("Function not found: {name:?}"));

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
                                "Invoke: {name}: Mismatch at index {i}: expected {expected:?}, got {actual:?} for directive #{directive_num} @ {linecol:?}"
                            );
                        }
                    }
                    _ => panic!("Unsupported exec directive: {exec:?} @ {linecol:?}"),
                },
                WastDirective::AssertMalformed {
                    mut module,
                    message,
                    ..
                } => {
                    let encoding = module.encode();
                    // WAST parser itself will toss certain malformed modules, which sorta defeats
                    // the purpose of this test, since it's just validating the encoder?
                    let encoding = match encoding {
                        Ok(e) => e,
                        Err(_r) => {
                            continue;
                        }
                    };
                    execution = TestModule::load(&encoding);
                    // There has to be a LoadFailed or LinkError for this to be malformed.
                    match execution {
                        TestModule::LoadFailed(_) | TestModule::LinkFailed(_) => {
                            // All good.
                        }
                        _ => panic!(
                            "Expected a load error w/ {message}, got {execution:?} for directive #{directive_num} @ {linecol:?}",
                        ),
                    }
                }
                WastDirective::Invoke(WastInvoke {
                    span: _,
                    module: _,
                    name,
                    args,
                }) => {
                    let TestModule::Loaded(ref mut execution) = execution else {
                        panic!("Expected a loaded module for invocation @ {linecol:?}");
                    };
                    let funcidx = execution
                        .instance()
                        .find_funcidx(name)
                        .unwrap_or_else(|| panic!("Function not found: {name:?}"));

                    let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                    execution.prepare(funcidx, &arg_set).unwrap();
                    execution.run().unwrap();
                    // For bare invoke, we don't check the result
                }
                WastDirective::AssertTrap { exec, message, .. } => match exec {
                    WastExecute::Invoke(WastInvoke {
                        span: _,
                        module: _,
                        name,
                        args,
                    }) => {
                        let TestModule::Loaded(ref mut execution) = execution else {
                            panic!("Expected a loaded module for invocation @ {linecol:?}");
                        };
                        let funcidx = execution
                            .instance()
                            .find_funcidx(name)
                            .unwrap_or_else(|| panic!("Function not found: {name:?}"));

                        let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                        execution.prepare(funcidx, &arg_set).unwrap();
                        let result = execution.run();

                        // We expect this to fail with a trap
                        match result {
                            Err(wasbox::ExecError::ExecutionFault(fault)) => {
                                let fault_message = fault.to_string();
                                let expected_message = message.to_string();
                                // Check if the fault message is compatible with the expected message
                                let is_compatible = match (expected_message.as_str(), fault_message.as_str()) {
                                    ("out of bounds memory access", "Memory out of bounds") => true,
                                    ("integer overflow", "integer overflow") => true,
                                    ("invalid conversion to integer", "invalid conversion to integer") => true,
                                    (expected, actual) if actual.contains(expected) => true,
                                    (expected, actual) if expected.contains(actual) => true,
                                    _ => false,
                                };
                                assert!(
                                    is_compatible,
                                    "Expected trap message '{}', got '{}' for directive #{directive_num} @ {linecol:?}",
                                    expected_message, fault_message
                                );
                            }
                            Ok(_) => panic!(
                                "Expected trap with message '{}', but execution succeeded for directive #{directive_num} @ {linecol:?}",
                                message
                            ),
                            Err(other) => panic!(
                                "Expected trap with message '{}', but got different error: {:?} for directive #{directive_num} @ {linecol:?}",
                                message, other
                            ),
                        }
                    }
                    _ => {
                        panic!("Unsupported exec directive in assert_trap: {exec:?} @ {linecol:?}")
                    }
                },
                _ => {}
            }
        }
    }

    // WAST test suite tests
    wast_test!(address_test, "address.wast");
    wast_test!(align_test, "align.wast");
    wast_test!(binary_test, "binary.wast");
    wast_test!(binary_leb128_test, "binary-leb128.wast");
    wast_test!(block_test, "block.wast");
    wast_test!(br_test, "br.wast");
    wast_test!(br_if_test, "br_if.wast");
    wast_test!(br_table_test, "br_table.wast");
    wast_test!(call_test, "call.wast");
    wast_test!(const_test, "const.wast");
    wast_test!(data_test, "data.wast");
    wast_test!(f32_test, "f32.wast");
    wast_test!(f32_bitwise_test, "f32_bitwise.wast");
    wast_test!(f64_test, "f64.wast");
    wast_test!(f64_bitwise_test, "f64_bitwise.wast");
    wast_test!(global_test, "global.wast");
    wast_test!(i32_test, "i32.wast");
    wast_test!(i64_test, "i64.wast");
    wast_test!(if_test, "if.wast");
    wast_test!(local_get_test, "local_get.wast");
    wast_test!(local_set_test, "local_set.wast");
    wast_test!(loop_test, "loop.wast");
    wast_test!(ref_func_test, "ref_func.wast");
    wast_test!(ref_is_null_test, "ref_is_null.wast");
    wast_test!(ref_null_test, "ref_null.wast");
    wast_test!(nop_test, "nop.wast");
    wast_test!(return_test, "return.wast");
    wast_test!(select_test, "select.wast");
    wast_test!(local_tee_test, "local_tee.wast");
    wast_test!(call_indirect_test, "call_indirect.wast");
    wast_test!(unreachable_test, "unreachable.wast");
    wast_test!(traps_test, "traps.wast");
    wast_test!(store_test, "store.wast");
    wast_test!(load_test, "load.wast");
    wast_test!(stack_test, "stack.wast");
    wast_test!(type_test, "type.wast");
    wast_test!(comments_test, "comments.wast");
    wast_test!(conversions_test, "conversions.wast");
    wast_test!(memory_test, "memory.wast");
    wast_test!(memory_size_test, "memory_size.wast");
    wast_test!(memory_grow_test, "memory_grow.wast");
    wast_test!(elem_test, "elem.wast");
    wast_test!(table_test, "table.wast");
    wast_test!(exports_test, "exports.wast");
    wast_test!(imports_test, "imports.wast");
    wast_test!(start_test, "start.wast");
    wast_test!(func_test, "func.wast");
    wast_test!(linking_test, "linking.wast");
    wast_test!(custom_test, "custom.wast");
    wast_test!(endianness_test, "endianness.wast");
    wast_test!(int_literals_test, "int_literals.wast");
    wast_test!(float_literals_test, "float_literals.wast");
    wast_test!(int_exprs_test, "int_exprs.wast");
    wast_test!(float_exprs_test, "float_exprs.wast");
    wast_test!(labels_test, "labels.wast");
    wast_test!(left_to_right_test, "left-to-right.wast");

    #[test]
    fn test_start_function_execution() {
        // WASM module with start function that increments memory 3 times
        let wasm_bytes = wat::parse_str(
            r#"
            (module
              (memory (data "A"))
              (func $inc
                (i32.store8
                  (i32.const 0)
                  (i32.add
                    (i32.load8_u (i32.const 0))
                    (i32.const 1)
                  )
                )
              )
              (func $get (result i32)
                (return (i32.load8_u (i32.const 0)))
              )
              (func $main
                (call $inc)
                (call $inc)
                (call $inc)
              )
              (start $main)
              (export "get" (func $get))
            )
        "#,
        )
        .unwrap();

        // Load and instantiate module (start function should run automatically)
        let module = wasbox::Module::load(&wasm_bytes).unwrap();
        let instance = wasbox::mk_instance(module).unwrap();

        // Create execution context and call the get function
        let memory = instance.memories[0].clone();
        let mut execution = wasbox::Execution::new(instance, memory);

        // Find the "get" function and call it
        let get_func_idx = execution.instance().find_funcidx("get").unwrap();
        execution.prepare(get_func_idx, &[]).unwrap();
        execution.run().unwrap();

        // Check that the result is 68 (65 + 3 increments)
        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], wasbox::Value::I32(68));
    }
}
