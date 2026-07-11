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
use rayrag::{pipeline::Pipeline, ParserConfig};
use tracing_subscriber;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Parse {
            file,
            chunk_size,
            overlap,
            embed,
            mode,
            api_base,
            api_key,
            api_model,
            model_path,
        } => {
            let config = ParserConfig {
                chunk_token_num: chunk_size,
                overlapped_percent: overlap,
                ..Default::default()
            };

            let mut pipeline = Pipeline::new(config);

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
                                model_path, model_path
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
                        let key = api_key
                            .ok_or_else(|| anyhow::anyhow!("--api-key or EMBED_API_KEY is required for openai mode"))?;
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
                if has_emb {
                    if let Some(ref emb) = chunk.embedding {
                        println!(
                            "  → embedding: [{:.3}, {:.3}, ..., {:.3}] ({}-dim)",
                            emb[0],
                            emb[1],
                            emb[emb.len() - 1],
                            emb.len()
                        );
                    }
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
            rayrag::server::run(port, &static_dir).await?;
            Ok(())
        }
    }
}
