use crate::domain::SubscriberEmail;
use reqwest::Client;

pub struct EmailClient {
    base_url: String,
    http_client: Client,
    sender: SubscriberEmail,
}

impl EmailClient {
    pub fn new(base_url: String, sender: SubscriberEmail) -> Self {
        Self {
            base_url,
            http_client: Client::new(),
            sender,
        }
    }

    pub async fn send_email(
        &self,
        recipient: SubscriberEmail,
        subject: &str,
        html_content: &str,
        text_content: &str,
    ) -> Result<(), String> {
        todo!()
    }
}
