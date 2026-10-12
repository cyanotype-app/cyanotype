use anyhow::Result;
use sqlx::{PgPool, postgres::PgPoolOptions};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(url: &str) -> Result<PgPool> {
    Ok(PgPoolOptions::new().max_connections(10).connect(url).await?)
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
