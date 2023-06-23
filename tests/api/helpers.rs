use nts::configuration::{get_configuration, DatabaseSettings};
use nts::startup::get_connection_pool;
use nts::startup::Application;
use nts::telemetry::{get_subscriber, init_subscriber};
use once_cell::sync::Lazy;
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;
use wiremock::MockServer;

static TRACING: Lazy<()> = Lazy::new(|| {
    let default_filter_level = "info".to_string();
    let subscriber_name = "test".to_string();
    if std::env::var("TEST_LOG").is_ok() {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::stdout);
        init_subscriber(subscriber);
    } else {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::sink);
        init_subscriber(subscriber);
    }
});

pub struct ConfirmationLinks {
    pub html: reqwest::Url,
    pub plain_text: reqwest::Url,
}

pub struct TestApp {
    pub address: String,
    pub db_pool: PgPool,
    database_settings: DatabaseSettings,
    pub email_server: MockServer,
    pub port: u16,
}

impl TestApp {
    pub async fn teardown_database(&self) {
        self.db_pool.close().await;

        let mut connection = PgConnection::connect(
            format!(
                "postgres://{}@{}:{}",
                &self.database_settings.username,
                &self.database_settings.host,
                self.database_settings.port
            )
            .as_str(),
        )
        .await
        .expect("Failed to connect to Postgres");

        connection
            .execute(
                format!(
                    r#"
                SELECT pg_terminate_backend(pg_stat_activity.pid)
                FROM pg_stat_activity
                WHERE pg_stat_activity.datname = '{}'
                AND pid <> pg_backend_pid();
                "#,
                    &self.database_settings.database_name
                )
                .as_str(),
            )
            .await
            .expect("Failed to drop connection to database");

        connection
            .execute(
                format!(
                    r#"DROP DATABASE "{}";"#,
                    &self.database_settings.database_name
                )
                .as_str(),
            )
            .await
            .expect("Failed to drop database");
    }

    pub async fn post_subscriptions(&self, body: String) -> reqwest::Response {
        reqwest::Client::new()
            .post(&format!("{}/subscriptions", &self.address))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .expect("Failed to execute request")
    }

    pub fn get_confirmation_links(&self, email_request: &wiremock::Request) -> ConfirmationLinks {
        let body: serde_json::Value = serde_json::from_slice(&email_request.body).unwrap();

        let get_link = |s: &str| {
            let links: Vec<_> = linkify::LinkFinder::new()
                .links(s)
                .filter(|l| *l.kind() == linkify::LinkKind::Url)
                .collect();
            assert_eq!(links.len(), 1);
            let raw_link = links[0].as_str().to_owned();
            let mut confirmation_link = reqwest::Url::parse(&raw_link).unwrap();
            assert_eq!(confirmation_link.host_str().unwrap(), "127.0.0.1");
            confirmation_link.set_port(Some(self.port)).unwrap();
            confirmation_link
        };

        let html = get_link(&body["HtmlBody"].as_str().unwrap());
        let plain_text = get_link(&body["TextBody"].as_str().unwrap());

        ConfirmationLinks { html, plain_text }
    }
}

pub async fn spawn_app() -> TestApp {
    Lazy::force(&TRACING);

    let email_server = MockServer::start().await;

    let configuration = {
        let mut c = get_configuration().expect("Failed to read configuration.");
        c.database.database_name = Uuid::new_v4().to_string();
        c.application.port = 0;
        c.email_client.base_url = email_server.uri();
        c
    };

    configure_database(&configuration.database).await;

    let application = Application::build(configuration.clone())
        .await
        .expect("Failed to build application.");
    let application_port = application.port();
    let _ = tokio::spawn(application.run_until_stopped());

    TestApp {
        address: format!("http://localhost:{}", application_port),
        db_pool: get_connection_pool(&configuration.database),
        database_settings: configuration.database,
        email_server,
        port: application_port,
    }
}

async fn configure_database(config: &DatabaseSettings) {
    let mut connection = PgConnection::connect_with(&config.without_db())
        .await
        .expect("Failed to connect to Postgres");
    connection
        .execute(format!(r#"CREATE DATABASE "{}";"#, config.database_name).as_str())
        .await
        .expect("Failed to create database");

    let connection_pool = PgPool::connect_with(config.with_db())
        .await
        .expect("Failed to connect to Postgres");

    // iterate over migration dir and execute query files
    let mut files: Vec<_> = Vec::new();
    let migration_dir = std::env::current_dir().unwrap().join("migrations");

    if let Ok(entries) = std::fs::read_dir(&migration_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                files.push(entry.file_name());
            } else {
                panic!("Failed to read directory entry");
            }
        }
    } else {
        panic!("Failed to read migration directory");
    }

    files.sort();

    for file in files.into_iter() {
        let query = std::fs::read_to_string(&migration_dir.join(file)).unwrap();
        connection_pool
            .execute(query.as_str())
            .await
            .expect("Failed to execute migration query");
    }
}
