use sqlx::PgPool;

use crate::structs::{Account, RegisterData};

const HASH_SALT: u32 = 12;
/// Tries to register to the DB, returns the user if successful.
/// Email must be unique and is enforced in the db.
pub async fn register_user(
    user_to_register: &RegisterData,
    pool: &PgPool,
) -> Result<Account, String> {
    let password_hash =
        bcrypt::hash(&user_to_register.password, HASH_SALT).map_err(|e| e.to_string())?;

    let db_user = sqlx::query_as!(
        Account,
        r#"INSERT INTO account (email, first_name, last_name, password, role, company_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, email, first_name, last_name, password, role as "role: _", company_id, created"#,
        user_to_register.email,
        user_to_register.first_name,
        user_to_register.last_name,
        password_hash,
        user_to_register.role as _,
        user_to_register.company_id
    ).fetch_one(pool).await.map_err(|e| e.to_string())?;
    // uniqueness is enforced at the database level.

    return Ok(db_user);
}
