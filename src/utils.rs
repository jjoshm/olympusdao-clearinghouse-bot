use anyhow::Result;
use ethers::utils::hex;
use reqwest::Client;
use std::time::SystemTime;

pub fn get_sys_time_in_secs() -> u64 {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(n) => n.as_secs(),
        Err(_) => panic!("SystemTime before UNIX EPOCH!"),
    }
}

pub async fn get_token_price(token: &str) -> Result<f64> {
    let web_client = Client::new();
    let url = format!("https://coins.llama.fi/prices/current/coingecko:{}", token);
    let payload = web_client
        .get(&url)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let price = payload["coins"][format!("coingecko:{}", token)]["price"]
        .as_f64()
        .unwrap();
    Ok(price)
}
