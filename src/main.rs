//! RayRAG CLI — document parsing + embedding pipeline.
//!
//! Usage:
//! ```bash
//! rayrag parse <file>                         # Parse + chunk only
//! rayrag parse <file> --embed --mode candle   # Local Candle model
//! rayrag parse <file> --embed --mode openai   # Cloud API (needs --api-base --api-key)
//! rayrag parse <file> --embed --mode hybrid   # Cloud first, local fallback
//! rayrag serve --port 9380                    # Start web server (SPA + API)
//! ```

use clap::{Parser, Subcommand, ValueEnum};
use rayrag::{Embedder, ParserConfig, pipeline::Pipeline, rerank::Reranker};

#[derive(Parser)]
#[command(name = "rayrag", version, about = "RAGFlow parsing in Rust + zvec")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse, chunk, and optionally embed a document
    Parse {
        /// Input file path
        file: String,

        /// Chunk token size (default: 2048)
        #[arg(long, default_value = "2048")]
        chunk_size: usize,

        /// Chunk overlap percentage (default: 0.05)
        #[arg(long, default_value = "0.05")]
        overlap: f32,

        /// Layout selector, including PaddleOCR/OpenDataLoader/MinerU/SoMark suffixes
        #[arg(long, default_value = "DeepDOC")]
        layout_recognize: String,

        /// Enable embedding generation
        #[arg(long)]
        embed: bool,

        /// Embedding mode: candle (local), openai (cloud), hybrid (cloud→local)
        #[arg(long, value_enum, default_value = "hybrid")]
        mode: EmbedMode,

        /// API base URL (for openai/hybrid mode)
        #[arg(long, default_value = "https://api.minimaxi.com/v1")]
        api_base: String,

        /// API key (for openai/hybrid mode)
        #[arg(long)]
        api_key: Option<String>,

        /// Model name for cloud API
        #[arg(long, default_value = "MiniMax-M3")]
        api_model: String,

        /// Local model directory (for candle/hybrid mode)
        #[arg(long, default_value = "models/all-MiniLM-L6-v2")]
        model_path: String,

        /// Persist embedded chunks in this collection
        #[arg(long)]
        collection: Option<String>,

        /// Portable JSON index path (ignored by the zvec backend)
        #[arg(long)]
        index: Option<String>,
    },

    /// Start the web server (static SPA + API)
    Serve {
        /// Port to listen on (default: 9380, same as RAGFlow)
        #[arg(long, default_value = "9380")]
        port: u16,

        /// Static files directory (default: web/static)
        #[arg(long, default_value = "web/static")]
        static_dir: String,
    },

    /// Online latency benchmark against a running RayRAG server
    /// (measures retrieval latency p50/p95/max over N iterations).
    Benchmark {
        /// RayRAG API base URL
        #[arg(long, default_value = "http://127.0.0.1:9390")]
        api_base: String,

        /// Admin email for login
        #[arg(long, default_value = "admin@rayrag.local")]
        email: String,

        /// Admin password (defaults to RAYRAG_ADMIN_PASSWORD env)
        #[arg(long)]
        password: Option<String>,

        /// Knowledge base id to query
        #[arg(long)]
        kb_id: String,

        /// Iterations per query (default: 5)
        #[arg(long, default_value = "5")]
        iterations: usize,

        /// Probe queries (repeatable; defaults to a built-in probe set)
        #[arg(long)]
        query: Vec<String>,

        /// Enable reranker on retrieval
        #[arg(long)]
        rerank: bool,
    },

    /// Search indexed documents using vector similarity
    Search {
        /// Search query text
        #[arg(short, long)]
        query: String,

        /// Path to JSON index file
        #[arg(short, long, default_value = "index.json")]
        index: String,

        /// Number of results to return (top-k)
        #[arg(long, default_value = "10")]
        top_k: usize,

        /// Embedding model path (for query embedding)
        #[arg(long, default_value = "models/all-MiniLM-L6-v2")]
        model_path: String,

        /// Use OpenAI API for embedding instead of local
        #[arg(long)]
        openai: bool,

        /// Enable re-ranking (uses local mxbai-rerank at :8899)
        #[arg(long)]
        rerank: bool,

        /// Reranker API URL (default: http://127.0.0.1:8899)
        #[arg(long, default_value = "http://127.0.0.1:8899")]
        rerank_url: String,
    },
}

#[derive(Clone, ValueEnum)]
enum EmbedMode {
    /// Local Candle model (all-MiniLM-L6-v2, ~23MB)
    Candle,
    /// Cloud API (OpenAI/MiniMax/vLLM compatible)
    Openai,
    /// Cloud first, local Candle fallback
    Hybrid,
}

/// Display plain cosine similarity results.
fn display_results(results: &[rayrag::search::SearchResult]) {
    println!("\n📊 Results ({}):\n", results.len());
    for result in results {
        println!(
            "#{}. [score: {:.4}] {} | {}",
            result.rank + 1,
            result.score,
            result.chunk.doc_name,
            result.chunk.id,
        );
        let preview = if result.chunk.content.len() > 150 {
            format!("{}...", &result.chunk.content[..150])
        } else {
            result.chunk.content.clone()
        };
        println!("   {}\n", preview);
    }
    if results.is_empty() {
        println!("   (no results found)");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let log_levels = rayrag::logging::init();
    // 统一命令超时输入：用户 env 环境文件（RAYRAG_CMD_TIMEOUT，默认 7200 秒/2 小时）
    rayrag::common::cmd_timeout::load_user_env_file();
    // Remember which file was read, so the first-login setup page writes back to it
    // rather than inventing a second one beside the state directory.
    rayrag::api::setup::remember_loaded_env_file();
    let cli = Cli::parse();

    match cli.command {
        Commands::Parse {
            file,
            chunk_size,
            overlap,
            layout_recognize,
            embed,
            mode,
            api_base,
            api_key,
            api_model,
            model_path,
            collection,
            index,
        } => {
            let config = ParserConfig {
                chunk_token_num: chunk_size,
                overlapped_percent: overlap,
                layout_recognize,
                ..Default::default()
            };

            let mut pipeline = Pipeline::new(config).with_document_parsers_from_env()?;

            if embed {
                let embedder: Box<dyn rayrag::embed::Embedder> = match mode {
                    EmbedMode::Candle => {
                        // Check if model exists locally first
                        let p = std::path::Path::new(&model_path);
                        if !p.join("model.safetensors").exists() {
                            anyhow::bail!(
                                "Candle model not found at: {}\n\
                                 \n\
                                 Models are NOT auto-downloaded. To download this model:\n\
                                 1. Open RayRAG UI and select \"Candle\" mode — this will trigger download\n\
                                 2. Or download manually from HuggingFace: sentence-transformers/all-MiniLM-L6-v2\n\
                                 \n\
                                 Expected files in '{}':\n\
                                   - model.safetensors\n\
                                   - config.json\n\
                                   - tokenizer.json",
                                model_path,
                                model_path
                            );
                        }
                        println!("🔧 Loading local model: {}", model_path);
                        Box::new(
                            rayrag::embed::CandleEmbedder::load(
                                rayrag::embed::CandleConfig::from_local(&model_path),
                            )
                            .await?,
                        )
                    }
                    EmbedMode::Openai => {
                        let key = api_key.ok_or_else(|| {
                            anyhow::anyhow!(
                                "--api-key or EMBED_API_KEY is required for openai mode"
                            )
                        })?;
                        println!("🌐 Using cloud API: {}", api_base);
                        Box::new(rayrag::embed::openai_compatible_embedder(
                            &api_base, &key, &api_model,
                        ))
                    }
                    EmbedMode::Hybrid => {
                        let key = api_key.unwrap_or_default();
                        if key.is_empty() {
                            println!("⚠️  No API key provided, using local Candle only");
                            Box::new(
                                rayrag::embed::CandleEmbedder::load(
                                    rayrag::embed::CandleConfig::from_local(&model_path),
                                )
                                .await?,
                            )
                        } else {
                            println!(
                                "🔄 Hybrid mode: {} → local Candle ({})",
                                api_base, model_path
                            );
                            let primary = Box::new(rayrag::embed::openai_compatible_embedder(
                                &api_base, &key, &api_model,
                            ));
                            let fallback = Box::new(
                                rayrag::embed::CandleEmbedder::load(
                                    rayrag::embed::CandleConfig::from_local(&model_path),
                                )
                                .await?,
                            );
                            Box::new(rayrag::embed::HybridEmbedder::new(primary, fallback))
                        }
                    }
                };
                pipeline = pipeline.with_embedder(embedder);
            }

            if let Some(collection) = collection {
                if !embed {
                    anyhow::bail!("--collection requires --embed");
                }
                let dimension = std::env::var("RAYRAG_EMBEDDING_DIMENSION")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(384);
                let store = if let Some(index) = index {
                    rayrag::store::ZvecStore::from_file(&collection, dimension, &index)?
                } else {
                    rayrag::store::ZvecStore::new(&collection, dimension)?
                };
                store.init()?;
                pipeline = pipeline.with_store(store);
            }

            let chunks = pipeline.process(&file).await?;

            println!("\n📄 Parsed: {}", file);
            println!("📦 Chunks: {}", chunks.len());
            for (i, chunk) in chunks.iter().enumerate() {
                let has_emb = chunk.embedding.is_some();
                println!(
                    "\n--- Chunk {} ({} tokens, embed:{}) ---",
                    i + 1,
                    chunk.token_count,
                    if has_emb { "✅" } else { "❌" }
                );
                let preview = if chunk.content.len() > 200 {
                    format!("{}...", &chunk.content[..200])
                } else {
                    chunk.content.clone()
                };
                println!("{}", preview);
                if has_emb && let Some(ref emb) = chunk.embedding {
                    println!(
                        "  → embedding: [{:.3}, {:.3}, ..., {:.3}] ({}-dim)",
                        emb[0],
                        emb[1],
                        emb[emb.len() - 1],
                        emb.len()
                    );
                }
            }
            Ok(())
        }
        Commands::Serve { port, static_dir } => {
            // Verify static directory exists
            let static_path = std::path::Path::new(&static_dir);
            if !static_path.exists() {
                anyhow::bail!(
                    "Static directory not found: {}\n\
                     Make sure the RAGFlow web frontend has been copied to web/static/\n\
                     Or specify a different path with --static-dir",
                    static_dir
                );
            }
            rayrag::server::run(port, &static_dir, log_levels).await?;
            Ok(())
        }
        Commands::Benchmark {
            api_base,
            email,
            password,
            kb_id,
            iterations,
            query,
            rerank,
        } => {
            use std::time::Instant;
            let password = password.unwrap_or_else(|| {
                std::env::var("RAYRAG_ADMIN_PASSWORD")
                    .expect("--password missing; set RAYRAG_ADMIN_PASSWORD env")
            });
            let client = rayrag::common::cmd_timeout::model_client();
            // 登录拿 token
            let login: serde_json::Value = client
                .post(format!("{api_base}/api/v1/auth/login"))
                .json(&serde_json::json!({ "email": email, "password": password }))
                .send()
                .await?
                .json()
                .await?;
            let token = login
                .get("data")
                .and_then(|d| d.get("access_token"))
                .and_then(|t| t.as_str())
                .ok_or_else(|| anyhow::anyhow!("login failed: {login}"))?;
            let auth = format!("Bearer {token}");
            // 探测查询（默认内置）
            let queries: Vec<String> = if query.is_empty() {
                vec![
                    "RAGFlow 是什么".into(),
                    "检索增强生成".into(),
                    "知识库 文档 解析".into(),
                    "embedding 模型".into(),
                ]
            } else {
                query
            };
            let mut samples: Vec<f64> = Vec::new();
            let mut failures = 0usize;
            for q in &queries {
                for _ in 0..iterations {
                    let t0 = Instant::now();
                    let resp = client
                        .post(format!("{api_base}/api/v1/retrieval"))
                        .header("Authorization", &auth)
                        .json(&serde_json::json!({
                            "question": q,
                            "kb_ids": [kb_id],
                            "top_k": 5,
                            "rerank": rerank,
                        }))
                        .send()
                        .await;
                    match resp {
                        Ok(r) if r.status().is_success() => {
                            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        _ => failures += 1,
                    }
                }
            }
            let stats = rayrag::benchmark::LatencyStats::from_millis(samples);
            println!("=== RayRAG retrieval latency benchmark ===");
            println!("api_base:     {api_base}");
            println!("kb_id:        {kb_id}");
            println!("queries:      {} × {} iter", queries.len(), iterations);
            println!("rerank:       {rerank}");
            println!("failures:     {failures}");
            println!(
                "count:        {}  avg: {:.1}ms  p50: {:.1}ms  p95: {:.1}ms  max: {:.1}ms",
                stats.count, stats.avg_ms, stats.p50_ms, stats.p95_ms, stats.max_ms
            );
            Ok(())
        }
        Commands::Search {
            query,
            index,
            top_k,
            model_path,
            openai,
            rerank,
            rerank_url,
        } => {
            // Load or create index
            let engine = if std::path::Path::new(&index).exists() {
                rayrag::search::SearchEngine::from_file(&index)?
            } else {
                anyhow::bail!(
                    "Index file not found: {}\n\
                     Build an index first:\n\
                       rayrag parse docs/*.pdf --embed --mode candle --model-path {}",
                    index,
                    model_path
                );
            };

            if engine.is_empty() {
                println!("⚠️  Index is empty — no chunks to search.");
                return Ok(());
            }

            // Embed the query
            println!("🔍 Searching for: \"{}\"", query);
            println!("📦 Index: {} chunks", engine.len());

            let query_embedding = if openai {
                anyhow::bail!("OpenAI mode not yet supported for search");
            } else {
                // Use local Candle embedder
                let p = std::path::Path::new(&model_path);
                if !p.join("model.safetensors").exists() {
                    anyhow::bail!(
                        "Candle model not found at: {}\n\
                         Download it via the RayRAG UI first.",
                        model_path
                    );
                }
                let embedder = rayrag::embed::CandleEmbedder::load(
                    rayrag::embed::CandleConfig::from_local(&model_path),
                )
                .await?;
                let embs = embedder.embed(&[&query]).await?;
                embs.into_iter().next().unwrap_or_default()
            };

            // Search
            let results = engine.search(&query_embedding, if rerank { top_k * 3 } else { top_k });

            // Apply reranking if requested
            if rerank && !results.is_empty() {
                println!("🔄 Re-ranking via {} ...", rerank_url);

                let reranker = rayrag::rerank::RemoteReranker::new(&rerank_url);
                let documents: Vec<String> =
                    results.iter().map(|r| r.chunk.content.clone()).collect();

                match reranker.rerank(&query, &documents, top_k).await {
                    Ok(reranked) => {
                        let final_results = rayrag::rerank::apply_rerank(results, &reranked);
                        println!("\n📊 Re-ranked Results ({}):\n", final_results.len());
                        for r in &final_results {
                            println!(
                                "#{}. [cosine: {:.4} | rerank: {:.4} | combined: {:.4}] {} | {}",
                                r.rank + 1,
                                r.score,
                                r.rerank_score,
                                r.combined_score,
                                r.doc_name,
                                r.chunk_id,
                            );
                            let preview = if r.content.len() > 150 {
                                format!("{}...", &r.content[..150])
                            } else {
                                r.content.clone()
                            };
                            println!("   {}\n", preview);
                        }
                        if final_results.is_empty() {
                            println!("   (no results after reranking)");
                        }
                    }
                    Err(e) => {
                        println!(
                            "⚠️  Reranker failed: {} — falling back to cosine results",
                            e
                        );
                        display_results(&results);
                    }
                }
            } else {
                display_results(&results);
            }

            Ok(())
        }
    }
}
