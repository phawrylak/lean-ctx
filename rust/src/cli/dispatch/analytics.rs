use crate::{core, tools};

pub(super) fn cmd_gain(rest: &[String]) {
    if rest.iter().any(|a| a == "--reset") {
        core::stats::reset_all();
        println!("Stats reset. All token savings data cleared.");
        return;
    }
    if rest.iter().any(|a| a == "--live" || a == "--watch") {
        core::stats::gain_live();
        return;
    }
    let model = rest.iter().enumerate().find_map(|(i, a)| {
        if let Some(v) = a.strip_prefix("--model=") {
            return Some(v.to_string());
        }
        if a == "--model" {
            return rest.get(i + 1).cloned();
        }
        None
    });
    let period = rest
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix("--period=") {
                return Some(v.to_string());
            }
            if a == "--period" {
                return rest.get(i + 1).cloned();
            }
            None
        })
        .unwrap_or_else(|| "all".to_string());
    let limit = rest
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix("--limit=") {
                return v.parse::<usize>().ok();
            }
            if a == "--limit" {
                return rest.get(i + 1).and_then(|v| v.parse::<usize>().ok());
            }
            None
        })
        .unwrap_or(10);

    if rest.iter().any(|a| a == "--graph") {
        println!("{}", core::stats::format_gain_graph());
    } else if rest.iter().any(|a| a == "--daily") {
        println!("{}", core::stats::format_gain_daily());
    } else if rest.iter().any(|a| a == "--json") {
        println!(
            "{}",
            tools::ctx_gain::handle("json", Some(&period), model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--score") {
        println!(
            "{}",
            tools::ctx_gain::handle("score", None, model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--cost") {
        println!(
            "{}",
            tools::ctx_gain::handle("cost", None, model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--tasks") {
        println!(
            "{}",
            tools::ctx_gain::handle("tasks", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--agents") {
        println!(
            "{}",
            tools::ctx_gain::handle("agents", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--heatmap") {
        println!(
            "{}",
            tools::ctx_gain::handle("heatmap", None, None, Some(limit))
        );
    } else if rest.iter().any(|a| a == "--wrapped") {
        println!(
            "{}",
            tools::ctx_gain::handle("wrapped", Some(&period), model.as_deref(), Some(limit))
        );
    } else if rest.iter().any(|a| a == "--pipeline") {
        let stats_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".lean-ctx")
            .join("pipeline_stats.json");
        if let Ok(data) = std::fs::read_to_string(&stats_path) {
            if let Ok(stats) = serde_json::from_str::<core::pipeline::PipelineStats>(&data) {
                println!("{}", stats.format_summary());
            } else {
                println!("No pipeline stats available yet (corrupt data).");
            }
        } else {
            println!("No pipeline stats available yet. Use MCP tools to generate data.");
        }
    } else if rest.iter().any(|a| a == "--deep") {
        println!(
            "{}\n{}\n{}\n{}\n{}",
            tools::ctx_gain::handle("report", None, model.as_deref(), Some(limit)),
            tools::ctx_gain::handle("tasks", None, None, Some(limit)),
            tools::ctx_gain::handle("cost", None, model.as_deref(), Some(limit)),
            tools::ctx_gain::handle("agents", None, None, Some(limit)),
            tools::ctx_gain::handle("heatmap", None, None, Some(limit))
        );
    } else {
        println!("{}", core::stats::format_gain());
    }
}

pub(super) fn cmd_graph(rest: &[String]) {
    let sub = rest.first().map_or("build", std::string::String::as_str);
    match sub {
        "build" => {
            let root = rest.get(1).cloned().or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            });
            let root = root.unwrap_or_else(|| ".".to_string());
            let index = core::graph_index::load_or_build(&root);
            println!(
                "Graph built: {} files, {} edges",
                index.files.len(),
                index.edges.len()
            );
        }
        "export-html" => {
            let mut root: Option<String> = None;
            let mut out: Option<String> = None;
            let mut max_nodes: usize = 2500;

            let args = &rest[1..];
            let mut i = 0usize;
            while i < args.len() {
                let a = args[i].as_str();
                if let Some(v) = a.strip_prefix("--root=") {
                    root = Some(v.to_string());
                } else if a == "--root" {
                    root = args.get(i + 1).cloned();
                    i += 1;
                } else if let Some(v) = a.strip_prefix("--out=") {
                    out = Some(v.to_string());
                } else if a == "--out" {
                    out = args.get(i + 1).cloned();
                    i += 1;
                } else if let Some(v) = a.strip_prefix("--max-nodes=") {
                    max_nodes = v.parse::<usize>().unwrap_or(0);
                } else if a == "--max-nodes" {
                    let v = args.get(i + 1).map_or("", String::as_str);
                    max_nodes = v.parse::<usize>().unwrap_or(0);
                    i += 1;
                }
                i += 1;
            }

            let root = root
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| ".".to_string());
            let Some(out) = out else {
                eprintln!("Usage: lean-ctx graph export-html --out <path> [--root <path>] [--max-nodes <n>]");
                std::process::exit(1);
            };
            if max_nodes == 0 {
                eprintln!("--max-nodes must be >= 1");
                std::process::exit(1);
            }

            core::graph_export::export_graph_html(&root, std::path::Path::new(&out), max_nodes)
                .unwrap_or_else(|e| {
                    eprintln!("graph export failed: {e}");
                    std::process::exit(1);
                });
            println!("{out}");
        }
        "related" | "impact" | "symbol" | "context" | "status" => {
            let path_arg = if sub == "status" {
                None
            } else {
                rest.get(1).map(String::as_str)
            };
            let root_idx = if sub == "status" { 1 } else { 2 };
            let root = resolve_graph_root(rest.get(root_idx));
            println!(
                "{}",
                tools::ctx_graph::handle(
                    sub,
                    path_arg,
                    &root,
                    &mut core::cache::SessionCache::new(),
                    tools::CrpMode::Off,
                    None,
                    None,
                )
            );
        }
        _ => {
            eprintln!(
                "Usage:\n  \
                 lean-ctx graph build [path]\n  \
                 lean-ctx graph related <file>\n  \
                 lean-ctx graph impact <file|symbol>\n  \
                 lean-ctx graph symbol <name>\n  \
                 lean-ctx graph context <query>\n  \
                 lean-ctx graph status\n  \
                 lean-ctx graph export-html --out <path> [--root <path>] [--max-nodes <n>]"
            );
            std::process::exit(1);
        }
    }
}

pub(super) fn cmd_smells(rest: &[String]) {
    let action = rest.first().map_or("summary", String::as_str);
    let rule = rest.iter().enumerate().find_map(|(i, a)| {
        if let Some(v) = a.strip_prefix("--rule=") {
            return Some(v.to_string());
        }
        if a == "--rule" {
            return rest.get(i + 1).cloned();
        }
        None
    });
    let path = rest.iter().enumerate().find_map(|(i, a)| {
        if let Some(v) = a.strip_prefix("--path=") {
            return Some(v.to_string());
        }
        if a == "--path" {
            return rest.get(i + 1).cloned();
        }
        None
    });
    let root = rest
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix("--root=") {
                return Some(v.to_string());
            }
            if a == "--root" {
                return rest.get(i + 1).cloned();
            }
            None
        })
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string());
    let fmt = if rest.iter().any(|a| a == "--json") {
        Some("json")
    } else {
        None
    };
    println!(
        "{}",
        tools::ctx_smells::handle(action, rule.as_deref(), path.as_deref(), &root, fmt)
    );
}

fn resolve_graph_root(arg: Option<&String>) -> String {
    arg.cloned()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string())
}

pub(super) fn cmd_compact(rest: &[String]) {
    let target = rest.first().map_or_else(
        || {
            let home = dirs::home_dir().unwrap_or_default();
            let claude = home.join(".claude").join("projects");
            if claude.is_dir() {
                claude
            } else {
                let cursor = home.join(".cursor").join("agent-transcripts");
                if cursor.is_dir() {
                    cursor
                } else {
                    std::env::current_dir().unwrap_or_default()
                }
            }
        },
        std::path::PathBuf::from,
    );

    if !target.exists() {
        eprintln!("Path does not exist: {}", target.display());
        std::process::exit(1);
    }

    let result = if target.is_file() {
        core::transcript_compact::compact_file(&target)
    } else {
        core::transcript_compact::compact_directory(&target)
    };

    match result {
        Ok(stats) => {
            println!("Transcript compaction: {stats}");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
