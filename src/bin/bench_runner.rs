use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use wasbox::bench_suite::{
    bench_wasbox, bench_wasbox_budgeted, bench_wasmtime, benchmark_cases, BenchCase,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Wasbox,
    Wasmtime,
    Both,
}

impl Engine {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "wasbox" => Ok(Self::Wasbox),
            "wasmtime" => Ok(Self::Wasmtime),
            "both" => Ok(Self::Both),
            _ => Err(format!(
                "invalid --engine value '{s}', expected wasbox|wasmtime|both"
            )),
        }
    }
}

#[derive(Debug)]
struct Cli {
    engine: Engine,
    warmup: usize,
    iters: usize,
    tick_slice: Option<usize>,
    cases: Vec<String>,
    list: bool,
    csv: Option<PathBuf>,
}

fn parse_args() -> Result<Cli, String> {
    let mut engine = Engine::Both;
    let mut warmup = 250;
    let mut iters = 2_000;
    let mut tick_slice = None;
    let mut cases = Vec::new();
    let mut list = false;
    let mut csv = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--engine" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--engine requires a value".to_string())?;
                engine = Engine::parse(&value)?;
            }
            "--warmup" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--warmup requires a value".to_string())?;
                warmup = value
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --warmup '{value}': {e}"))?;
            }
            "--iters" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--iters requires a value".to_string())?;
                iters = value
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --iters '{value}': {e}"))?;
            }
            "--tick-slice" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--tick-slice requires a value".to_string())?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --tick-slice '{value}': {e}"))?;
                if parsed == 0 {
                    return Err("--tick-slice must be > 0".to_string());
                }
                tick_slice = Some(parsed);
            }
            "--case" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--case requires a value".to_string())?;
                cases.push(value);
            }
            "--csv" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--csv requires a value".to_string())?;
                csv = Some(PathBuf::from(value));
            }
            "--list" => list = true,
            "-h" | "--help" => {
                println!(
                    "Usage: cargo run --release --bin bench_runner -- [options]\n\
                    \n\
                    Options:\n\
                      --engine wasbox|wasmtime|both   Engine to run (default: both)\n\
                      --warmup N                       Warmup iterations (default: 250)\n\
                      --iters N                        Timed iterations (default: 2000)\n\
                      --tick-slice N                   Run wasbox via run_for_ticks(N)\n\
                      --case NAME                      Run a specific case (repeatable)\n\
                      --list                           List available benchmark cases\n\
                      --csv PATH                       Write CSV output\n"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unrecognized argument '{other}'")),
        }
    }

    Ok(Cli {
        engine,
        warmup,
        iters,
        tick_slice,
        cases,
        list,
        csv,
    })
}

fn selected_cases<'a>(
    cases: &'a [BenchCase],
    selected: &[String],
) -> Result<Vec<&'a BenchCase>, String> {
    if selected.is_empty() {
        return Ok(cases.iter().collect());
    }
    let wanted: HashSet<&str> = selected.iter().map(|s| s.as_str()).collect();
    let picked: Vec<&BenchCase> = cases.iter().filter(|c| wanted.contains(c.name)).collect();
    if picked.len() != wanted.len() {
        let mut missing = Vec::new();
        for name in wanted {
            if !cases.iter().any(|c| c.name == name) {
                missing.push(name.to_string());
            }
        }
        missing.sort();
        return Err(format!("unknown case(s): {}", missing.join(", ")));
    }
    Ok(picked)
}

fn main() -> Result<(), String> {
    let cli = parse_args()?;
    let cases = benchmark_cases();

    if cli.list {
        for c in cases {
            println!("{}", c.name);
        }
        return Ok(());
    }

    let selected = selected_cases(cases, &cli.cases)?;
    if selected.is_empty() {
        return Err("no benchmark cases selected".to_string());
    }

    let mut csv_rows = vec![
        "case,engine,warmup,iters,instantiate_ms,exec_total_ms,exec_mean_ns,last_value".to_string(),
    ];
    println!(
        "{:<20} {:<9} {:>10} {:>14} {:>14} {:>12}",
        "case", "engine", "init_ms", "exec_total_ms", "exec_mean_ns", "last"
    );

    for case in selected {
        let run_wasbox = matches!(cli.engine, Engine::Wasbox | Engine::Both);
        let run_wasmtime = matches!(cli.engine, Engine::Wasmtime | Engine::Both);

        if run_wasbox {
            let (r, engine_label) = if let Some(tick_slice) = cli.tick_slice {
                (
                    bench_wasbox_budgeted(case, cli.warmup, cli.iters, tick_slice)?,
                    format!("wasbox@{tick_slice}"),
                )
            } else {
                (
                    bench_wasbox(case, cli.warmup, cli.iters)?,
                    "wasbox".to_string(),
                )
            };
            println!(
                "{:<20} {:<9} {:>10.3} {:>14.3} {:>14.1} {:>12}",
                case.name,
                engine_label,
                r.instantiate.as_secs_f64() * 1_000.0,
                r.execute_total.as_secs_f64() * 1_000.0,
                r.mean_execute_ns(),
                r.last_value
            );
            csv_rows.push(format!(
                "{},{},{},{},{:.6},{:.6},{:.1},{}",
                case.name,
                engine_label,
                cli.warmup,
                cli.iters,
                r.instantiate.as_secs_f64() * 1_000.0,
                r.execute_total.as_secs_f64() * 1_000.0,
                r.mean_execute_ns(),
                r.last_value
            ));
        }

        if run_wasmtime {
            let r = bench_wasmtime(case, cli.warmup, cli.iters)?;
            println!(
                "{:<20} {:<9} {:>10.3} {:>14.3} {:>14.1} {:>12}",
                case.name,
                "wasmtime",
                r.instantiate.as_secs_f64() * 1_000.0,
                r.execute_total.as_secs_f64() * 1_000.0,
                r.mean_execute_ns(),
                r.last_value
            );
            csv_rows.push(format!(
                "{},{},{},{},{:.6},{:.6},{:.1},{}",
                case.name,
                "wasmtime",
                cli.warmup,
                cli.iters,
                r.instantiate.as_secs_f64() * 1_000.0,
                r.execute_total.as_secs_f64() * 1_000.0,
                r.mean_execute_ns(),
                r.last_value
            ));
        }
    }

    if let Some(path) = cli.csv {
        fs::write(&path, csv_rows.join("\n"))
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        println!("\nWrote CSV: {}", path.display());
    }

    Ok(())
}
