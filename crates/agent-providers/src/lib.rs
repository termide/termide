//! Model providers for the termide coding agent.
//!
//! The first and, for local models, usually the only provider is
//! [`OpenAiCompatProvider`]: streaming chat completions as served by
//! llama.cpp, Ollama, vLLM, omlx, OpenRouter and most gateways. Vendor quirks
//! are expressed as data in [`Compat`] rather than as code paths.
//!
//! HTTP is blocking `ureq` on the agent's worker thread, consistent with the
//! rest of termide (no async runtime); cancellation is polled between SSE
//! lines.

mod anthropic;
mod openai;
mod retry;
mod slots;
mod sse;

pub use anthropic::AnthropicProvider;
pub use openai::{Compat, OpenAiCompatProvider, ReasoningParam};
pub use retry::RetryPolicy;
pub use slots::{Permit, Slots, SlottedProvider};
