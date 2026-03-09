use crate::{mk_instance, ExecutionState, Module, Value};
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchValue {
    I32(i32),
    I64(i64),
}

impl Display for BenchValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchValue::I32(v) => write!(f, "{v}"),
            BenchValue::I64(v) => write!(f, "{v}"),
        }
    }
}

impl BenchValue {
    fn into_wasbox(self) -> Value {
        match self {
            BenchValue::I32(v) => Value::I32(v),
            BenchValue::I64(v) => Value::I64(v),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchCase {
    pub name: &'static str,
    pub wat: &'static str,
    pub invoke: &'static str,
    pub params: &'static [BenchValue],
    pub expected: Option<BenchValue>,
}

pub fn benchmark_cases() -> &'static [BenchCase] {
    &[
        BenchCase {
            name: "sum_i64_10000",
            wat: r#"(module
                (func (export "run") (param i64) (result i64)
                    (local i64)
                    (i64.const 0)
                    (local.set 1)
                    (block
                        (loop
                            (local.get 0)
                            (i64.eqz)
                            (br_if 1)
                            (local.get 1)
                            (local.get 0)
                            (i64.add)
                            (local.set 1)
                            (local.get 0)
                            (i64.const 1)
                            (i64.sub)
                            (local.set 0)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I64(10_000)],
            expected: Some(BenchValue::I64(50_005_000)),
        },
        BenchCase {
            name: "lcg_i32_100000",
            wat: r#"(module
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (i32.const 1664525)
                            (i32.mul)
                            (i32.const 1013904223)
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(100_000)],
            expected: None,
        },
        BenchCase {
            name: "indirect_mix_50000",
            wat: r#"(module
                (type $f (func (param i32) (result i32)))
                (func $f0 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 3)
                    (i32.mul)
                    (i32.const 7)
                    (i32.add))
                (func $f1 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 5)
                    (i32.mul)
                    (i32.const 11)
                    (i32.add))
                (table 2 funcref)
                (elem (i32.const 0) $f0 $f1)
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (local.get 2)
                            (local.get 2)
                            (i32.const 1)
                            (i32.and)
                            (call_indirect (type $f))
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(50_000)],
            expected: None,
        },
        BenchCase {
            name: "direct_call_mix_50000",
            wat: r#"(module
                (func $f0 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 3)
                    (i32.mul)
                    (i32.const 7)
                    (i32.add))
                (func $f1 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 5)
                    (i32.mul)
                    (i32.const 11)
                    (i32.add))
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.and)
                            (if (result i32)
                              (then
                                (local.get 2)
                                (call $f1))
                              (else
                                (local.get 2)
                                (call $f0)))
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(50_000)],
            expected: None,
        },
        BenchCase {
            name: "direct_mono_50000",
            wat: r#"(module
                (func $f0 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 3)
                    (i32.mul)
                    (i32.const 7)
                    (i32.add))
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (local.get 2)
                            (call $f0)
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(50_000)],
            expected: Some(BenchValue::I32(-544_692_296)),
        },
        BenchCase {
            name: "inline_mono_50000",
            wat: r#"(module
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (local.get 2)
                            (i32.const 3)
                            (i32.mul)
                            (i32.const 7)
                            (i32.add)
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(50_000)],
            expected: Some(BenchValue::I32(-544_692_296)),
        },
        BenchCase {
            name: "indirect_mono_50000",
            wat: r#"(module
                (type $f (func (param i32) (result i32)))
                (func $f0 (param i32) (result i32)
                    (local.get 0)
                    (i32.const 3)
                    (i32.mul)
                    (i32.const 7)
                    (i32.add))
                (table 1 funcref)
                (elem (i32.const 0) $f0)
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 2)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (local.get 2)
                            (i32.const 0)
                            (call_indirect (type $f))
                            (i32.add)
                            (local.set 1)
                            (local.get 2)
                            (i32.const 1)
                            (i32.add)
                            (local.set 2)
                            (br 0)
                        )
                    )
                    (local.get 1)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(50_000)],
            expected: None,
        },
        BenchCase {
            name: "mem_store_load_sum_8192",
            wat: r#"(module
                (memory 1)
                (func (export "run") (param i32) (result i32)
                    (local i32 i32)
                    (i32.const 0)
                    (local.set 1)
                    (block
                        (loop
                            (local.get 1)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (i32.const 2)
                            (i32.shl)
                            (local.get 1)
                            (i32.store)
                            (local.get 1)
                            (i32.const 1)
                            (i32.add)
                            (local.set 1)
                            (br 0)
                        )
                    )
                    (i32.const 0)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (block
                        (loop
                            (local.get 1)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 2)
                            (local.get 1)
                            (i32.const 2)
                            (i32.shl)
                            (i32.load)
                            (i32.add)
                            (local.set 2)
                            (local.get 1)
                            (i32.const 1)
                            (i32.add)
                            (local.set 1)
                            (br 0)
                        )
                    )
                    (local.get 2)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(8_192)],
            expected: Some(BenchValue::I32(33_550_336)),
        },
        BenchCase {
            name: "branchy_lcg_if_100000",
            wat: r#"(module
                (func (export "run") (param i32) (result i32)
                    (local i32 i32 i32)
                    (i32.const 1)
                    (local.set 1)
                    (i32.const 0)
                    (local.set 2)
                    (i32.const 0)
                    (local.set 3)
                    (block
                        (loop
                            (local.get 3)
                            (local.get 0)
                            (i32.ge_u)
                            (br_if 1)
                            (local.get 1)
                            (i32.const 1664525)
                            (i32.mul)
                            (i32.const 1013904223)
                            (i32.add)
                            (local.set 1)
                            (local.get 1)
                            (i32.const 1)
                            (i32.and)
                            (if
                              (then
                                (local.get 2)
                                (local.get 3)
                                (i32.add)
                                (local.set 2))
                              (else
                                (local.get 2)
                                (local.get 3)
                                (i32.sub)
                                (local.set 2)))
                            (local.get 3)
                            (i32.const 1)
                            (i32.add)
                            (local.set 3)
                            (br 0)
                        )
                    )
                    (local.get 2)
                    (local.get 1)
                    (i32.xor)
                )
            )"#,
            invoke: "run",
            params: &[BenchValue::I32(100_000)],
            expected: None,
        },
    ]
}

#[derive(Debug, Clone, Copy)]
pub struct BenchResult {
    pub instantiate: Duration,
    pub execute_total: Duration,
    pub iters: usize,
    pub last_value: BenchValue,
}

impl BenchResult {
    pub fn mean_execute_ns(&self) -> f64 {
        if self.iters == 0 {
            return 0.0;
        }
        self.execute_total.as_nanos() as f64 / self.iters as f64
    }
}

pub struct WasboxHarness {
    exec: ExecutionState,
    funcidx: u32,
    args: Vec<Value>,
}

impl WasboxHarness {
    pub fn new(case: &BenchCase) -> Result<(Self, Duration), String> {
        let wasm_bytes = wat::parse_str(case.wat)
            .map_err(|e| format!("{}: failed to parse WAT: {e}", case.name))?;
        let t0 = Instant::now();
        let module = Module::load(&wasm_bytes)
            .map_err(|e| format!("{}: failed to load module: {e}", case.name))?;
        let instance = mk_instance(module)
            .map_err(|e| format!("{}: failed to instantiate: {e}", case.name))?;
        let funcidx = instance
            .find_funcidx(case.invoke)
            .ok_or_else(|| format!("{}: missing export {}", case.name, case.invoke))?;
        let mut exec = ExecutionState::new();
        exec.add_module(0, instance);
        let instantiate = t0.elapsed();

        let args = case.params.iter().map(|v| v.into_wasbox()).collect();
        Ok((
            Self {
                exec,
                funcidx,
                args,
            },
            instantiate,
        ))
    }

    pub fn invoke_once(&mut self) -> Result<BenchValue, String> {
        self.exec
            .prepare(0, self.funcidx, &self.args)
            .map_err(|e| format!("prepare failed: {e}"))?;
        self.exec.run().map_err(|e| format!("run failed: {e}"))?;
        let result = self
            .exec
            .result()
            .and_then(|v| v.first())
            .ok_or_else(|| "no result returned".to_string())?;
        match result {
            Value::I32(v) => Ok(BenchValue::I32(*v)),
            Value::I64(v) => Ok(BenchValue::I64(*v)),
            other => Err(format!("unsupported result type: {other:?}")),
        }
    }

    pub fn invoke_once_budgeted(&mut self, tick_slice: usize) -> Result<BenchValue, String> {
        if tick_slice == 0 {
            return Err("tick_slice must be > 0".to_string());
        }
        self.exec
            .prepare(0, self.funcidx, &self.args)
            .map_err(|e| format!("prepare failed: {e}"))?;
        loop {
            match self
                .exec
                .run_for_ticks(tick_slice)
                .map_err(|e| format!("run_for_ticks failed: {e}"))?
            {
                crate::RunResult::Paused => continue,
                crate::RunResult::Complete => break,
            }
        }
        let result = self
            .exec
            .result()
            .and_then(|v| v.first())
            .ok_or_else(|| "no result returned".to_string())?;
        match result {
            Value::I32(v) => Ok(BenchValue::I32(*v)),
            Value::I64(v) => Ok(BenchValue::I64(*v)),
            other => Err(format!("unsupported result type: {other:?}")),
        }
    }
}

pub fn bench_wasbox(case: &BenchCase, warmup: usize, iters: usize) -> Result<BenchResult, String> {
    let (mut harness, instantiate) = WasboxHarness::new(case)?;
    for _ in 0..warmup {
        let _ = harness.invoke_once()?;
    }
    let t0 = Instant::now();
    let mut last = BenchValue::I32(0);
    for _ in 0..iters {
        last = harness.invoke_once()?;
    }
    if let Some(expected) = case.expected {
        if last != expected {
            return Err(format!(
                "{}: result mismatch (expected {}, got {})",
                case.name, expected, last
            ));
        }
    }
    Ok(BenchResult {
        instantiate,
        execute_total: t0.elapsed(),
        iters,
        last_value: last,
    })
}

pub fn bench_wasbox_budgeted(
    case: &BenchCase,
    warmup: usize,
    iters: usize,
    tick_slice: usize,
) -> Result<BenchResult, String> {
    let (mut harness, instantiate) = WasboxHarness::new(case)?;
    for _ in 0..warmup {
        let _ = harness.invoke_once_budgeted(tick_slice)?;
    }
    let t0 = Instant::now();
    let mut last = BenchValue::I32(0);
    for _ in 0..iters {
        last = harness.invoke_once_budgeted(tick_slice)?;
    }
    if let Some(expected) = case.expected {
        if last != expected {
            return Err(format!(
                "{}: result mismatch (expected {}, got {})",
                case.name, expected, last
            ));
        }
    }
    Ok(BenchResult {
        instantiate,
        execute_total: t0.elapsed(),
        iters,
        last_value: last,
    })
}

pub fn bench_wasmtime(
    case: &BenchCase,
    warmup: usize,
    iters: usize,
) -> Result<BenchResult, String> {
    use wasmtime::{Engine, Instance, Module, Store, Val};

    let wasm_bytes =
        wat::parse_str(case.wat).map_err(|e| format!("{}: failed to parse WAT: {e}", case.name))?;
    let engine = Engine::default();
    let t0 = Instant::now();
    let module = Module::new(&engine, &wasm_bytes)
        .map_err(|e| format!("{}: module error: {e}", case.name))?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[])
        .map_err(|e| format!("{}: instance error: {e}", case.name))?;
    let func = instance
        .get_func(&mut store, case.invoke)
        .ok_or_else(|| format!("{}: missing export {}", case.name, case.invoke))?;
    let result_arity = func.ty(&store).results().len();
    let instantiate = t0.elapsed();

    let params: Vec<Val> = case
        .params
        .iter()
        .map(|v| match v {
            BenchValue::I32(x) => Val::I32(*x),
            BenchValue::I64(x) => Val::I64(*x),
        })
        .collect();
    let mut results = vec![Val::I32(0); result_arity];

    for _ in 0..warmup {
        func.call(&mut store, &params, &mut results)
            .map_err(|e| format!("warmup call failed: {e}"))?;
    }

    let t1 = Instant::now();
    for _ in 0..iters {
        func.call(&mut store, &params, &mut results)
            .map_err(|e| format!("call failed: {e}"))?;
    }
    let execute_total = t1.elapsed();

    let last_value = match results.first() {
        Some(Val::I32(v)) => BenchValue::I32(*v),
        Some(Val::I64(v)) => BenchValue::I64(*v),
        Some(other) => return Err(format!("unsupported result type: {other:?}")),
        None => return Err("no result returned".to_string()),
    };
    if let Some(expected) = case.expected {
        if last_value != expected {
            return Err(format!(
                "{}: result mismatch (expected {}, got {})",
                case.name, expected, last_value
            ));
        }
    }

    Ok(BenchResult {
        instantiate,
        execute_total,
        iters,
        last_value,
    })
}
