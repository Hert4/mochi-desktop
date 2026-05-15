use claude_code_rust::agent::llama_client::{ChatEvent, LlamaConfig, Message, Role, stream_chat};
use futures::StreamExt as _;
use std::io::Write as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8765".to_owned());
    let prompt = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Say hello in one short sentence.".to_owned());

    let config = LlamaConfig {
        url,
        model: None,
        temperature: Some(0.7),
        max_tokens: Some(128),
    };

    let messages = vec![
        Message { role: Role::System, content: "You are Mochi, a cute terminal pet.".to_owned() },
        Message { role: Role::User, content: prompt },
    ];

    eprintln!("→ POST {}/v1/chat/completions", config.url);
    eprintln!("→ user: {}", messages[1].content);
    eprint!("← ");

    let mut stream = Box::pin(stream_chat(&config, &messages).await?);
    let mut total = 0usize;
    while let Some(item) = stream.next().await {
        match item? {
            ChatEvent::Delta(text) => {
                print!("{text}");
                std::io::stdout().flush().ok();
                total += text.len();
            }
            ChatEvent::Done => break,
        }
    }
    println!();
    eprintln!("[done, {total} chars streamed]");
    Ok(())
}
