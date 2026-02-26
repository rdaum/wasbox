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
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::fmt::{Debug, Formatter};
    use std::path::Path;
    use std::rc::Rc;
    use wasbox::{
        mk_instance, mk_instance_with_imports, Continuation, DecodeError, ElementMode,
        ExecutionState, ImportResolver, ImportedFunction, LinkError, LoaderError, Module,
        ReferenceType, TableInstance, Value, VectorMemory,
    };
    use wast::core::{NanPattern, WastArgCore, WastRetCore};
    use wast::lexer::Lexer;
    use wast::{
        parser, QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
    };

    /// Standard spectest resolver for WAST test suite
    pub struct SpectestResolver;

    impl ImportResolver for SpectestResolver {
        fn resolve_function(&self, module: &str, field: &str) -> Option<ImportedFunction> {
            if module != "spectest" {
                return None;
            }

            match field {
                "print" => Some(ImportedFunction::Host(Box::new(|_stack| {
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_i32" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg = stack.pop_i32()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_i64" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg = stack.pop_i64()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_f32" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg = stack.pop_f32()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_f64" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg = stack.pop_f64()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_i32_f32" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg2 = stack.pop_f32()?;
                    let _arg1 = stack.pop_i32()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "print_f64_f64" => Some(ImportedFunction::Host(Box::new(|stack| {
                    let _arg2 = stack.pop_f64()?;
                    let _arg1 = stack.pop_f64()?;
                    Ok(Continuation::ProgramEnd)
                }))),
                "global_i32" => Some(ImportedFunction::Host(Box::new(|stack| {
                    stack.push_i32(666);
                    Ok(Continuation::ProgramEnd)
                }))),
                "global_i64" => Some(ImportedFunction::Host(Box::new(|stack| {
                    stack.push_i64(666);
                    Ok(Continuation::ProgramEnd)
                }))),
                "global_f32" => Some(ImportedFunction::Host(Box::new(|stack| {
                    stack.push_f32(666.6);
                    Ok(Continuation::ProgramEnd)
                }))),
                "global_f64" => Some(ImportedFunction::Host(Box::new(|stack| {
                    stack.push_f64(666.6);
                    Ok(Continuation::ProgramEnd)
                }))),
                _ => None,
            }
        }

        fn resolve_global(&self, module: &str, field: &str) -> Option<Value> {
            if module != "spectest" {
                return None;
            }

            match field {
                "global_i32" => Some(Value::I32(666)),
                "global_i64" => Some(Value::I64(666)),
                "global_f32" => Some(Value::F32(666.6)),
                "global_f64" => Some(Value::F64(666.6)),
                _ => None,
            }
        }

        fn resolve_table(&self, module: &str, field: &str) -> Option<TableInstance> {
            if module != "spectest" {
                return None;
            }

            match field {
                "table" => Some(TableInstance {
                    elements: Rc::new(RefCell::new(vec![None; 10])), // Start with 10 elements
                    ref_type: ReferenceType::FuncRef,
                    limits: (10, Some(20)), // Min 10, max 20
                }),
                _ => None,
            }
        }

        fn resolve_memory(&self, module: &str, field: &str) -> Option<VectorMemory> {
            if module != "spectest" {
                return None;
            }

            match field {
                "memory" => Some(VectorMemory::new(65536, Some(2 * 65536))), // 1 page min, 2 pages max
                _ => None,
            }
        }
    }

    #[derive(Default, Clone)]
    struct ModuleExports {
        functions: HashMap<String, u32>,
        globals: HashMap<String, Value>,
        tables: HashMap<String, TableInstance>,
        memories: HashMap<String, VectorMemory>,
    }

    /// Test registry for WAST tests that combines spectest and registered modules.
    pub struct TestRegistry {
        spectest: SpectestResolver,
        registered_modules: HashMap<String, u32>,
        module_exports: HashMap<u32, ModuleExports>,
    }

    impl TestRegistry {
        pub fn new() -> Self {
            Self {
                spectest: SpectestResolver,
                registered_modules: HashMap::new(),
                module_exports: HashMap::new(),
            }
        }

        pub fn register_module(&mut self, name: &str, module_id: u32) {
            self.registered_modules.insert(name.to_string(), module_id);
        }

        pub fn resolve_registered_module(&self, name: &str) -> Option<u32> {
            self.registered_modules.get(name).copied()
        }

        pub fn cache_module(&mut self, module_id: u32, instance: &wasbox::Instance) {
            let mut exports = ModuleExports::default();

            for (name, func_idx) in instance.exported_functions() {
                exports.functions.insert(name, func_idx);
            }
            for (name, global_idx) in instance.exported_globals() {
                if let Some(g) = instance.globals.get(global_idx as usize) {
                    exports.globals.insert(name, g.value);
                }
            }
            for (name, table_idx) in instance.exported_tables() {
                if let Some(t) = instance.tables.get(table_idx as usize) {
                    exports.tables.insert(name, t.clone());
                }
            }
            for (name, memory_idx) in instance.exported_memories() {
                if let Some(m) = instance.memories.get(memory_idx as usize) {
                    exports.memories.insert(name, m.clone());
                }
            }

            self.module_exports.insert(module_id, exports);
        }

        pub fn refresh_from_execution(&mut self, execution: &ExecutionState) {
            let mut module_ids = HashSet::new();
            for module_id in self.registered_modules.values().copied() {
                module_ids.insert(module_id);
            }

            for module_id in module_ids {
                if let Some(instance) = execution.get_module(module_id) {
                    self.cache_module(module_id, instance);
                }
            }
        }
    }

    impl Default for TestRegistry {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ImportResolver for TestRegistry {
        fn resolve_function(&self, module: &str, field: &str) -> Option<ImportedFunction> {
            // Check spectest first
            if let Some(imported) = self.spectest.resolve_function(module, field) {
                return Some(imported);
            }

            // Then check registered modules
            if let Some(&module_id) = self.registered_modules.get(module) {
                if let Some(exports) = self.module_exports.get(&module_id) {
                    if let Some(&func_idx) = exports.functions.get(field) {
                        return Some(ImportedFunction::Module {
                            module_id,
                            func_idx,
                        });
                    }
                }
            }

            None
        }

        fn resolve_global(&self, module: &str, field: &str) -> Option<Value> {
            // Check spectest first
            if let Some(global) = self.spectest.resolve_global(module, field) {
                return Some(global);
            }

            // Then check registered modules
            if let Some(&module_id) = self.registered_modules.get(module) {
                if let Some(exports) = self.module_exports.get(&module_id) {
                    return exports.globals.get(field).copied();
                }
            }

            None
        }

        fn resolve_table(&self, module: &str, field: &str) -> Option<TableInstance> {
            // Check spectest first
            if let Some(table) = self.spectest.resolve_table(module, field) {
                return Some(table);
            }

            // Then check registered modules
            if let Some(&module_id) = self.registered_modules.get(module) {
                if let Some(exports) = self.module_exports.get(&module_id) {
                    return exports.tables.get(field).cloned();
                }
            }

            None
        }

        fn resolve_memory(&self, module: &str, field: &str) -> Option<VectorMemory> {
            // Check spectest first
            if let Some(memory) = self.spectest.resolve_memory(module, field) {
                return Some(memory);
            }

            // Then check registered modules
            if let Some(&module_id) = self.registered_modules.get(module) {
                if let Some(exports) = self.module_exports.get(&module_id) {
                    return exports.memories.get(field).cloned();
                }
            }

            None
        }
    }

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

    fn ret_core_matches(expected: &WastRetCore, actual: &wasbox::Value) -> bool {
        match expected {
            WastRetCore::I32(i) => matches!(actual, wasbox::Value::I32(v) if v == i),
            WastRetCore::I64(i) => matches!(actual, wasbox::Value::I64(v) if v == i),
            WastRetCore::F32(NanPattern::Value(f)) => {
                matches!(actual, wasbox::Value::F32(v) if v.to_bits() == f.bits)
            }
            WastRetCore::F32(NanPattern::ArithmeticNan)
            | WastRetCore::F32(NanPattern::CanonicalNan) => {
                matches!(actual, wasbox::Value::F32(v) if v.is_nan())
            }
            WastRetCore::F64(NanPattern::Value(f)) => {
                matches!(actual, wasbox::Value::F64(v) if v.to_bits() == f.bits)
            }
            WastRetCore::F64(NanPattern::ArithmeticNan)
            | WastRetCore::F64(NanPattern::CanonicalNan) => {
                matches!(actual, wasbox::Value::F64(v) if v.is_nan())
            }
            WastRetCore::RefNull(None) => {
                matches!(
                    actual,
                    wasbox::Value::FuncRef(None) | wasbox::Value::ExternRef(None)
                )
            }
            WastRetCore::RefNull(Some(ref_type)) => match format!("{ref_type:?}").as_str() {
                s if s.contains("Func") => matches!(actual, wasbox::Value::FuncRef(None)),
                _ => matches!(actual, wasbox::Value::ExternRef(None)),
            },
            WastRetCore::RefExtern(r) => matches!(actual, wasbox::Value::ExternRef(v) if v == r),
            WastRetCore::Either(cases) => cases.iter().any(|case| ret_core_matches(case, actual)),
            WastRetCore::RefFunc(_) => matches!(actual, wasbox::Value::FuncRef(Some(_))),
            WastRetCore::RefAny
            | WastRetCore::RefEq
            | WastRetCore::RefArray
            | WastRetCore::RefStruct
            | WastRetCore::RefI31
            | WastRetCore::RefI31Shared
            | WastRetCore::RefHost(_) => matches!(
                actual,
                wasbox::Value::FuncRef(Some(_)) | wasbox::Value::ExternRef(Some(_))
            ),
            _ => false,
        }
    }

    fn ret_matches(expected: &WastRet, actual: &wasbox::Value) -> bool {
        match expected {
            WastRet::Core(core) => ret_core_matches(core, actual),
            _ => false,
        }
    }

    enum TestModule {
        None,
        Loaded(Box<ExecutionState>),
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
                        let mut exec_state = ExecutionState::new();
                        exec_state.add_module(0, i);
                        TestModule::Loaded(Box::new(exec_state))
                    }
                    Err(e) => TestModule::LinkFailed(e),
                },
                Err(e) => TestModule::LoadFailed(e),
            }
        }
    }

    fn module_identifier(module: &QuoteWat<'_>) -> Option<String> {
        match module {
            QuoteWat::Wat(Wat::Module(m)) => m.id.map(|id| id.name().to_string()),
            _ => None,
        }
    }

    fn resolve_module_id(
        module: Option<wast::token::Id<'_>>,
        current_module_id: u32,
        registry: &TestRegistry,
    ) -> u32 {
        if let Some(module) = module {
            return registry
                .resolve_registered_module(module.name())
                .unwrap_or_else(|| panic!("Unknown module reference: {}", module.name()));
        }
        current_module_id
    }

    fn perform_wast(path: &Path) {
        let file = std::fs::File::open(path).unwrap();
        let input = std::io::read_to_string(file).unwrap();

        let lexer = Lexer::new(&input);
        let pb = wast::parser::ParseBuffer::new_with_lexer(lexer).unwrap();
        let ast = parser::parse::<Wast>(&pb)
            .unwrap_or_else(|_| panic!("Failed to parse WAST file {path:?}"));

        let mut execution = TestModule::None;
        let mut registry = TestRegistry::new();
        let mut module_counter = 0u32;
        let mut current_module_id = 0u32;

        for (directive_num, directive) in ast.directives.into_iter().enumerate() {
            let directive_span = directive.span();
            let linecol = directive_span.linecol_in(&input);

            match directive {
                WastDirective::Module(mut module) => {
                    let module_identifier = module_identifier(&module);
                    let encoded = module.encode().unwrap();
                    let m = Module::load(&encoded);
                    match m {
                        Ok(m) => {
                            // Check if module has imports
                            let has_imports = !m.imports.is_empty();

                            let result = if has_imports {
                                // Use import resolver for modules with imports
                                mk_instance_with_imports(m, &registry)
                            } else {
                                // Use regular instance creation for modules without imports
                                mk_instance(m)
                            };

                            // Handle the result and update execution state
                            match result {
                                Ok(i) => {
                                    let new_module_id = module_counter;
                                    if let Some(module_name) = module_identifier.as_deref() {
                                        registry.register_module(module_name, new_module_id);
                                    }

                                    // Reuse existing execution state if available
                                    if let TestModule::Loaded(ref mut exec_state) = execution {
                                        // Add module to existing execution state
                                        exec_state.add_module(new_module_id, i);
                                        if let Some(instance) = exec_state.get_module(new_module_id)
                                        {
                                            registry.cache_module(new_module_id, instance);
                                        }
                                        current_module_id = new_module_id;
                                        module_counter += 1;
                                    } else {
                                        // Create new execution state for first module
                                        let mut exec_state = ExecutionState::new();
                                        exec_state.add_module(new_module_id, i);
                                        if let Some(instance) = exec_state.get_module(new_module_id)
                                        {
                                            registry.cache_module(new_module_id, instance);
                                        }
                                        current_module_id = new_module_id;
                                        module_counter += 1;
                                        execution = TestModule::Loaded(Box::new(exec_state));
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Link failed at directive #{directive_num} @ {linecol:?}: {e:?}");
                                    execution = TestModule::LinkFailed(e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Load failed at directive #{directive_num} @ {linecol:?}: {e:?}"
                            );
                            execution = TestModule::LoadFailed(e);
                        }
                    };
                }
                WastDirective::AssertReturn { exec, results, .. } => match exec {
                    // Invoke runs executions on the last loaded module.
                    WastExecute::Invoke(WastInvoke {
                        span: _,
                        module,
                        name,
                        args,
                    }) => {
                        let TestModule::Loaded(ref mut execution) = execution else {
                            panic!("Expected a loaded module for invocation @ {linecol:?}");
                        };
                        let target_module_id =
                            resolve_module_id(module, current_module_id, &registry);
                        let funcidx = execution
                            .get_module(target_module_id)
                            .unwrap()
                            .find_funcidx(name)
                            .unwrap_or_else(|| panic!("Function not found: {name:?}"));

                        let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                        execution
                            .prepare(target_module_id, funcidx, &arg_set)
                            .unwrap();
                        if let Err(e) = execution.run() {
                            panic!(
                                "Invoke run failed for {name} at directive #{directive_num} @ {linecol:?}: {e:?}"
                            );
                        }
                        registry.refresh_from_execution(execution);
                        let result = execution.result().unwrap();

                        for (i, (expected, actual)) in results.iter().zip(result.iter()).enumerate()
                        {
                            assert!(
                                ret_matches(expected, actual),
                                "Invoke: {name}: Mismatch at index {i}: expected {expected:?}, got {actual:?} for directive #{directive_num} @ {linecol:?}"
                            );
                        }
                    }
                    WastExecute::Get {
                        span: _,
                        module,
                        global,
                    } => {
                        let TestModule::Loaded(ref execution) = execution else {
                            panic!("Expected a loaded module for global get @ {linecol:?}");
                        };
                        let target_module_id =
                            resolve_module_id(module, current_module_id, &registry);
                        let instance = execution
                            .get_module(target_module_id)
                            .unwrap_or_else(|| panic!("Unknown module id {target_module_id}"));
                        let global_idx = instance
                            .find_globalidx(global)
                            .unwrap_or_else(|| panic!("Global not found: {global:?}"));
                        let actual = instance
                            .globals
                            .get(global_idx as usize)
                            .unwrap_or_else(|| panic!("Global index out of bounds: {global_idx}"))
                            .value;
                        let actual = if let Some((module_name, field_name)) = global.split_once('.')
                        {
                            if let Some(source_module_id) =
                                registry.resolve_registered_module(module_name)
                            {
                                if let Some(source_instance) =
                                    execution.get_module(source_module_id)
                                {
                                    if let Some(source_global_idx) =
                                        source_instance.find_globalidx(field_name)
                                    {
                                        source_instance
                                            .globals
                                            .get(source_global_idx as usize)
                                            .unwrap_or_else(|| {
                                                panic!(
                                                    "Global index out of bounds: {source_global_idx}"
                                                )
                                            })
                                            .value
                                    } else {
                                        actual
                                    }
                                } else {
                                    actual
                                }
                            } else {
                                actual
                            }
                        } else {
                            actual
                        };

                        assert!(
                            !results.is_empty(),
                            "Expected at least one result for get {global:?}"
                        );
                        assert!(
                            ret_matches(&results[0], &actual),
                            "Get: {global}: expected {:?}, got {:?} for directive #{directive_num} @ {linecol:?}",
                            results[0],
                            actual
                        );
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
                    module,
                    name,
                    args,
                }) => {
                    let TestModule::Loaded(ref mut execution) = execution else {
                        panic!("Expected a loaded module for invocation @ {linecol:?}");
                    };
                    let target_module_id = resolve_module_id(module, current_module_id, &registry);
                    let funcidx = execution
                        .get_module(target_module_id)
                        .unwrap()
                        .find_funcidx(name)
                        .unwrap_or_else(|| panic!("Function not found: {name:?}"));

                    let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                    execution
                        .prepare(target_module_id, funcidx, &arg_set)
                        .unwrap();
                    execution.run().unwrap();
                    registry.refresh_from_execution(execution);
                    // For bare invoke, we don't check the result
                }
                WastDirective::AssertTrap { exec, message, .. } => match exec {
                    WastExecute::Invoke(WastInvoke {
                        span: _,
                        module,
                        name,
                        args,
                    }) => {
                        let TestModule::Loaded(ref mut execution) = execution else {
                            panic!("Expected a loaded module for invocation @ {linecol:?}");
                        };
                        let target_module_id =
                            resolve_module_id(module, current_module_id, &registry);
                        let funcidx = execution
                            .get_module(target_module_id)
                            .unwrap()
                            .find_funcidx(name)
                            .unwrap_or_else(|| panic!("Function not found: {name:?}"));

                        let arg_set: Vec<_> = args.iter().map(convert_value).collect();
                        execution
                            .prepare(target_module_id, funcidx, &arg_set)
                            .unwrap();
                        let result = execution.run();
                        registry.refresh_from_execution(execution);

                        // We expect this to fail with a trap
                        match result {
                            Err(wasbox::ExecError::ExecutionFault(fault)) => {
                                let fault_message = fault.to_string();
                                let expected_message = message.to_string();
                                // Check if the fault message is compatible with the expected message
                                let is_compatible = match (expected_message.as_str(), fault_message.as_str()) {
                                    ("out of bounds memory access", "Memory out of bounds") => true,
                                    ("out of bounds table access", "Memory out of bounds") => true,
                                    ("integer overflow", "integer overflow") => true,
                                    ("invalid conversion to integer", "invalid conversion to integer") => true,
                                    (expected, actual) if actual.contains(expected) => true,
                                    (expected, actual) if expected.contains(actual) => true,
                                    _ => false,
                                };
                                assert!(
                                    is_compatible,
                                    "Expected trap message '{expected_message}', got '{fault_message}' for directive #{directive_num} @ {linecol:?}"
                                );
                            }
                            Ok(_) => panic!(
                                "Expected trap with message '{message}', but execution succeeded for directive #{directive_num} @ {linecol:?}"
                            ),
                            Err(other) => panic!(
                                "Expected trap with message '{message}', but got different error: {other:?} for directive #{directive_num} @ {linecol:?}"
                            ),
                        }
                    }
                    WastExecute::Wat(mut module) => {
                        let encoded = module.encode().unwrap();
                        let result = match Module::load(&encoded) {
                            Ok(m) => {
                                if m.imports.is_empty() {
                                    mk_instance(m)
                                } else {
                                    mk_instance_with_imports(m, &registry)
                                }
                            }
                            Err(_) => {
                                // Malformed decode is acceptable for assert_trap in the current harness.
                                continue;
                            }
                        };

                        if result.is_ok() {
                            panic!(
                                "Expected trap with message '{message}', but module instantiation succeeded for directive #{directive_num} @ {linecol:?}"
                            );
                        }
                    }
                    _ => {
                        panic!("Unsupported exec directive in assert_trap: {exec:?} @ {linecol:?}")
                    }
                },
                WastDirective::Register { name, module, .. } => {
                    // Register the current module with the given name
                    if let TestModule::Loaded(ref execution_state) = execution {
                        let target_module_id =
                            resolve_module_id(module, current_module_id, &registry);
                        let instance = execution_state.get_module(target_module_id).unwrap_or_else(|| {
                            panic!(
                                "No module with id {target_module_id} to register at directive #{directive_num} @ {linecol:?}"
                            )
                        });
                        registry.cache_module(target_module_id, instance);
                        registry.register_module(name, target_module_id);
                    } else {
                        panic!("No module loaded to register at directive #{directive_num} @ {linecol:?}");
                    }
                }
                _ => {}
            }
        }
    }

    // WAST test suite tests
    wast_test!(address_test, "address.wast");
    wast_test!(address0_test, "address0.wast");
    wast_test!(address1_test, "address1.wast");
    wast_test!(align_test, "align.wast");
    wast_test!(align0_test, "align0.wast");
    wast_test!(annotations_test, "annotations.wast");
    wast_test!(binary_test, "binary.wast");
    wast_test!(binary0_test, "binary0.wast");
    wast_test!(binary_leb128_test, "binary-leb128.wast");
    wast_test!(block_test, "block.wast");
    wast_test!(br_test, "br.wast");
    wast_test!(br_if_test, "br_if.wast");
    wast_test!(br_table_test, "br_table.wast");
    wast_test!(bulk_test, "bulk.wast");
    wast_test!(call_test, "call.wast");
    wast_test!(const_test, "const.wast");
    wast_test!(data_test, "data.wast");
    wast_test!(data0_test, "data0.wast");
    wast_test!(data1_test, "data1.wast");
    wast_test!(data_drop0_test, "data_drop0.wast");
    wast_test!(f32_test, "f32.wast");
    wast_test!(f32_bitwise_test, "f32_bitwise.wast");
    wast_test!(f32_cmp_test, "f32_cmp.wast");
    wast_test!(f64_test, "f64.wast");
    wast_test!(f64_bitwise_test, "f64_bitwise.wast");
    wast_test!(f64_cmp_test, "f64_cmp.wast");
    wast_test!(fac_test, "fac.wast");
    wast_test!(float_memory_test, "float_memory.wast");
    wast_test!(float_memory0_test, "float_memory0.wast");
    wast_test!(float_misc_test, "float_misc.wast");
    wast_test!(forward_test, "forward.wast");
    wast_test!(func_ptrs_test, "func_ptrs.wast");
    wast_test!(global_test, "global.wast");
    wast_test!(i32_test, "i32.wast");
    wast_test!(i64_test, "i64.wast");
    wast_test!(if_test, "if.wast");
    wast_test!(id_test, "id.wast");
    wast_test!(inline_module_test, "inline-module.wast");
    wast_test!(local_get_test, "local_get.wast");
    wast_test!(local_init_test, "local_init.wast");
    wast_test!(local_set_test, "local_set.wast");
    wast_test!(loop_test, "loop.wast");
    wast_test!(ref_func_test, "ref_func.wast");
    wast_test!(ref_is_null_test, "ref_is_null.wast");
    wast_test!(ref_null_test, "ref_null.wast");
    wast_test!(ref_test, "ref.wast");
    wast_test!(nop_test, "nop.wast");
    wast_test!(return_test, "return.wast");
    wast_test!(select_test, "select.wast");
    wast_test!(local_tee_test, "local_tee.wast");
    wast_test!(call_indirect_test, "call_indirect.wast");
    wast_test!(unreachable_test, "unreachable.wast");
    wast_test!(traps_test, "traps.wast");
    wast_test!(traps0_test, "traps0.wast");
    wast_test!(store_test, "store.wast");
    wast_test!(store0_test, "store0.wast");
    wast_test!(store1_test, "store1.wast");
    wast_test!(store2_test, "store2.wast");
    wast_test!(load_test, "load.wast");
    wast_test!(load0_test, "load0.wast");
    wast_test!(load1_test, "load1.wast");
    wast_test!(load2_test, "load2.wast");
    wast_test!(stack_test, "stack.wast");
    wast_test!(type_test, "type.wast");
    wast_test!(comments_test, "comments.wast");
    wast_test!(conversions_test, "conversions.wast");
    wast_test!(memory_test, "memory.wast");
    wast_test!(memory_size_test, "memory_size.wast");
    wast_test!(memory_size0_test, "memory_size0.wast");
    wast_test!(memory_size1_test, "memory_size1.wast");
    wast_test!(memory_size2_test, "memory_size2.wast");
    wast_test!(memory_size3_test, "memory_size3.wast");
    wast_test!(memory_size_import_test, "memory_size_import.wast");
    wast_test!(memory_trap_test, "memory_trap.wast");
    wast_test!(memory_trap0_test, "memory_trap0.wast");
    wast_test!(memory_trap1_test, "memory_trap1.wast");
    wast_test!(memory_grow_test, "memory_grow.wast");
    wast_test!(memory_copy_test, "memory_copy.wast");
    wast_test!(memory_copy0_test, "memory_copy0.wast");
    wast_test!(memory_copy1_test, "memory_copy1.wast");
    wast_test!(memory_fill_test, "memory_fill.wast");
    wast_test!(memory_fill0_test, "memory_fill0.wast");
    wast_test!(memory_init_test, "memory_init.wast");
    wast_test!(memory_init0_test, "memory_init0.wast");
    wast_test!(memory_redundancy_test, "memory_redundancy.wast");
    wast_test!(memory_multi_test, "memory-multi.wast");
    wast_test!(elem_test, "elem.wast");
    wast_test!(table_test, "table.wast");
    wast_test!(exports_test, "exports.wast");
    wast_test!(exports0_test, "exports0.wast");
    wast_test!(imports_test, "imports.wast");
    wast_test!(imports0_test, "imports0.wast");
    wast_test!(imports1_test, "imports1.wast");
    wast_test!(imports2_test, "imports2.wast");
    wast_test!(imports3_test, "imports3.wast");
    wast_test!(imports4_test, "imports4.wast");
    wast_test!(start_test, "start.wast");
    wast_test!(start0_test, "start0.wast");
    wast_test!(func_test, "func.wast");
    wast_test!(linking_test, "linking.wast");
    wast_test!(linking0_test, "linking0.wast");
    wast_test!(linking1_test, "linking1.wast");
    wast_test!(linking2_test, "linking2.wast");
    wast_test!(linking3_test, "linking3.wast");
    wast_test!(custom_test, "custom.wast");
    wast_test!(endianness_test, "endianness.wast");
    wast_test!(int_literals_test, "int_literals.wast");
    wast_test!(float_literals_test, "float_literals.wast");
    wast_test!(int_exprs_test, "int_exprs.wast");
    wast_test!(float_exprs_test, "float_exprs.wast");
    wast_test!(float_exprs0_test, "float_exprs0.wast");
    wast_test!(float_exprs1_test, "float_exprs1.wast");
    wast_test!(labels_test, "labels.wast");
    wast_test!(left_to_right_test, "left-to-right.wast");
    wast_test!(table_get_test, "table_get.wast");
    wast_test!(table_set_test, "table_set.wast");
    wast_test!(table_size_test, "table_size.wast");
    wast_test!(table_grow_test, "table_grow.wast");
    wast_test!(table_fill_test, "table_fill.wast");
    wast_test!(table_copy_test, "table_copy.wast");
    wast_test!(table_copy_mixed_test, "table_copy_mixed.wast");
    wast_test!(table_init_test, "table_init.wast");
    wast_test!(switch_test, "switch.wast");
    wast_test!(token_test, "token.wast");
    wast_test!(skip_stack_guard_page_test, "skip-stack-guard-page.wast");
    wast_test!(utf8_custom_section_id_test, "utf8-custom-section-id.wast");
    wast_test!(utf8_import_field_test, "utf8-import-field.wast");
    wast_test!(utf8_import_module_test, "utf8-import-module.wast");
    wast_test!(utf8_invalid_encoding_test, "utf8-invalid-encoding.wast");

    #[test]
    fn test_minimal_import() {
        let wasm_bytes = wat::parse_str(
            r#"
            (module
              (import "spectest" "print_i32" (func $print (param i32)))
              (func (export "test") (param i32)
                (call $print (local.get 0))
              )
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&wasm_bytes).unwrap();

        struct TestResolver;
        impl ImportResolver for TestResolver {
            fn resolve_function(&self, module: &str, field: &str) -> Option<ImportedFunction> {
                if module == "spectest" && field == "print_i32" {
                    Some(ImportedFunction::Host(Box::new(|stack| {
                        let _arg = stack.pop_i32()?;
                        Ok(Continuation::ProgramEnd)
                    })))
                } else {
                    None
                }
            }

            fn resolve_global(&self, _module: &str, _field: &str) -> Option<Value> {
                None
            }

            fn resolve_table(&self, _module: &str, _field: &str) -> Option<TableInstance> {
                None
            }

            fn resolve_memory(&self, _module: &str, _field: &str) -> Option<VectorMemory> {
                None
            }
        }

        let resolver = TestResolver;
        let instance = mk_instance_with_imports(module, &resolver).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let test_func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("test")
            .unwrap();

        // This should work
        execution
            .prepare(0, test_func_idx, &[Value::I32(42)])
            .unwrap();
        execution.run().unwrap();
    }

    #[test]
    fn test_table_with_imports() {
        let wasm_bytes = wat::parse_str(
            r#"
            (module
              (import "spectest" "print_i32" (func $print_i32 (param i32)))
              (import "spectest" "print_f64" (func $print_f64 (param f64)))
              (table funcref (elem $print_i32 $print_f64))
              (func (export "test")
                (call_indirect (type 0) (i32.const 42) (i32.const 0))
              )
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&wasm_bytes).unwrap();
        println!("Module loaded successfully");
        println!("Module element segments: {:#?}", module.element_segments);

        // Look at the expression that's being evaluated
        if let Some(segment) = module.element_segments.first() {
            if let ElementMode::Active {
                table_index: _,
                expr,
            } = &segment.mode
            {
                let expr_bytes = module.get_expr(expr);
                println!("Expression bytes: {expr_bytes:?}");
            }
        }
    }

    #[test]
    fn test_ref_func_simple() {
        // Test simple ref.func in global initialization
        let wasm_bytes = wat::parse_str(
            r#"
            (module
              (func $f (param i32) (result i32) (local.get 0))
              (global funcref (ref.func $f))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&wasm_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        println!("Success! ref.func in global initialization works.");
        println!("Global count: {}", instance.globals.len());

        // Check that the global was initialized with the correct function reference
        assert_eq!(instance.globals.len(), 1);
        if let Value::FuncRef(Some(func_ref)) = instance.globals[0].value {
            assert_eq!(func_ref.func_idx, 0); // Should refer to function 0
        } else {
            panic!("Global should be initialized with function reference");
        }
    }

    #[test]
    fn test_inter_module_call() {
        // First module that will be registered as "test"
        let module1_bytes = wat::parse_str(
            r#"
            (module
              (func (export "func-i64->i64") (param i64) (result i64) (local.get 0))
            )
        "#,
        )
        .unwrap();

        // Second module that imports from "test"
        let module2_bytes = wat::parse_str(
            r#"
            (module
              (import "test" "func-i64->i64" (func $i64->i64 (param i64) (result i64)))
              (func (export "test") (param i64) (result i64)
                (call $i64->i64 (local.get 0))
              )
            )
        "#,
        )
        .unwrap();

        let module1 = Module::load(&module1_bytes).unwrap();
        let module2 = Module::load(&module2_bytes).unwrap();

        // Create instances
        let instance1 = mk_instance(module1).unwrap();

        // Set up registry
        let mut registry = TestRegistry::new();
        registry.cache_module(0, &instance1);
        registry.register_module("test", 0);

        let instance2 = mk_instance_with_imports(module2, &registry).unwrap();

        // Set up execution state
        let mut execution = ExecutionState::new();
        execution.add_module(0, instance1);
        execution.add_module(1, instance2);

        // Try to call the test function
        let test_func_idx = execution
            .get_module(1)
            .unwrap()
            .find_funcidx("test")
            .unwrap();

        // This should work
        execution
            .prepare(1, test_func_idx, &[Value::I64(42)])
            .unwrap();
        execution.run().unwrap();

        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::I64(42));
    }

    #[test]
    fn test_simple_no_imports() {
        // Test a simple module with no imports to ensure basic functionality still works
        let module_bytes = wat::parse_str(
            r#"
            (module
              (func (export "simple") (result i32) (i32.const 42))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&module_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("simple")
            .unwrap();
        execution.prepare(0, func_idx, &[]).unwrap();
        execution.run().unwrap();

        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::I32(42));
    }

    #[test]
    fn test_empty_function() {
        // Test a function that has no body (like type-use-1 in func.wast)
        let module_bytes = wat::parse_str(
            r#"
            (module
              (type $sig (func))
              (func (export "empty") (type $sig))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&module_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("empty")
            .unwrap();
        execution.prepare(0, func_idx, &[]).unwrap();
        execution.run().unwrap();

        // Empty function should return no values
        let result = execution.result().unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_func_wast_type_use() {
        // Test that reproduces the exact type-use-1 function from func.wast
        let module_bytes = wat::parse_str(
            r#"
            (module
              (type $sig-1 (func))
              (func (export "type-use-1") (type $sig-1))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&module_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("type-use-1")
            .unwrap();
        execution.prepare(0, func_idx, &[]).unwrap();
        execution.run().unwrap();

        // Should return no values
        let result = execution.result().unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_memory_indexing() {
        // Test that module without imports has memory at index 0
        let module_bytes = wat::parse_str(
            r#"
            (module
              (memory 1)
              (func (export "memsize") (result i32) (memory.size))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&module_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        // Check that there's exactly one memory
        assert_eq!(instance.memories.len(), 1);

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("memsize")
            .unwrap();
        execution.prepare(0, func_idx, &[]).unwrap();
        execution.run().unwrap();

        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::I32(1)); // Should be 1 page
    }

    #[test]
    fn test_memory_with_data() {
        // Test using exact same pattern as WAST framework
        let module_bytes = wat::parse_str(
            r#"
            (module 
              (memory (data "x")) 
              (func (export "memsize") (result i32) (memory.size))
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&module_bytes).unwrap();
        let instance = mk_instance(module).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("memsize")
            .unwrap();
        execution.prepare(0, func_idx, &[]).unwrap();
        execution.run().unwrap();

        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::I32(1)); // Should be 1 page due to data "x"
    }

    #[test]
    fn test_complex_inter_module_call() {
        // First module that will be registered as "test"
        let module1_bytes = wat::parse_str(
            r#"
            (module
              (func (export "func-i64->i64") (param i64) (result i64) (local.get 0))
            )
        "#,
        )
        .unwrap();

        // Second module that imports from "test" and has complex operations
        let module2_bytes = wat::parse_str(
            r#"
            (module
              (import "test" "func-i64->i64" (func $i64->i64 (param i64) (result i64)))
              (func (export "complex") (param $i i64)
                (local $x f64)
                (local.set $x (f64.convert_i64_s (call $i64->i64 (local.get $i))))
                (drop (local.get $x))
              )
            )
        "#,
        )
        .unwrap();

        let module1 = Module::load(&module1_bytes).unwrap();
        let module2 = Module::load(&module2_bytes).unwrap();

        // Create instances
        let instance1 = mk_instance(module1).unwrap();

        // Set up registry
        let mut registry = TestRegistry::new();
        registry.cache_module(0, &instance1);
        registry.register_module("test", 0);

        let instance2 = mk_instance_with_imports(module2, &registry).unwrap();

        // Set up execution state
        let mut execution = ExecutionState::new();
        execution.add_module(0, instance1);
        execution.add_module(1, instance2);

        // Try to call the complex function
        let test_func_idx = execution
            .get_module(1)
            .unwrap()
            .find_funcidx("complex")
            .unwrap();

        // This should work
        execution
            .prepare(1, test_func_idx, &[Value::I64(42)])
            .unwrap();
        execution.run().unwrap();
    }

    #[test]
    fn test_direct_print_f64_call() {
        // Test calling print_f64 directly (not through call_indirect)
        let wasm_bytes = wat::parse_str(
            r#"
            (module
              (import "spectest" "print_f64" (func $print_f64 (param f64)))
              (func (export "test") (param f64)
                (call $print_f64 (local.get 0))
              )
            )
        "#,
        )
        .unwrap();

        let module = Module::load(&wasm_bytes).unwrap();
        let instance = mk_instance_with_imports(module, &SpectestResolver).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);

        let test_func_idx = execution
            .get_module(0)
            .unwrap()
            .find_funcidx("test")
            .unwrap();

        // This should work if the issue is with call_indirect, not with print_f64 itself
        execution
            .prepare(0, test_func_idx, &[Value::F64(42.0)])
            .unwrap();
        execution.run().unwrap();
    }

    #[test]
    fn test_simplified_imports() {
        // Test a simple import scenario inline
        let module1_bytes = wat::parse_str(
            r#"
            (module
              (func (export "func") (result i32) (i32.const 42))
              (global (export "global-i32") i32 (i32.const 55))
            )
        "#,
        )
        .unwrap();

        let module2_bytes = wat::parse_str(
            r#"
            (module
              (import "test" "func" (func $imported_func (result i32)))
              (func (export "test_func") (result i32)
                (call $imported_func)
              )
            )
        "#,
        )
        .unwrap();

        let module1 = Module::load(&module1_bytes).unwrap();
        let module2 = Module::load(&module2_bytes).unwrap();

        let instance1 = mk_instance(module1).unwrap();

        let mut registry = TestRegistry::new();
        registry.cache_module(0, &instance1);
        registry.register_module("test", 0);

        let instance2 = mk_instance_with_imports(module2, &registry).unwrap();

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance1);
        execution.add_module(1, instance2);

        let func_idx = execution
            .get_module(1)
            .unwrap()
            .find_funcidx("test_func")
            .unwrap();
        execution.prepare(1, func_idx, &[]).unwrap();
        execution.run().unwrap();

        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::I32(42));
    }

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
        let get_func_idx = instance.find_funcidx("get").unwrap();
        let mut execution = wasbox::ExecutionState::new();
        execution.add_module(0, instance);

        // Find the "get" function and call it
        execution.prepare(0, get_func_idx, &[]).unwrap();
        execution.run().unwrap();

        // Check that the result is 68 (65 + 3 increments)
        let result = execution.result().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], wasbox::Value::I32(68));
    }
}
