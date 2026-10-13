//! Transaction helpers shared by the store modules. Callers must have
//! `StoreError` and `store_error` in scope, or name their own error type and
//! sqlx error mapper (the delivery journal does).

/// One write transaction on the writer lease. The body plans against the
/// open transaction; the lease is consumed only after the transaction is
/// committed or rolled back, as schedule commits do. A failed BEGIN, COMMIT
/// or ROLLBACK (or a cancelled call) orphans the connection.
macro_rules! write_txn {
    ($store:expr, $tx:ident, $body:expr) => {
        write_txn!(StoreError, store_error; $store, $tx, $body)
    };
    ($error:ident, $sql_error:path; $store:expr, $tx:ident, $body:expr) => {{
        let mut lease = $store
            .writer_lease()
            .await
            .map_err(|error| $error::Backend(error.to_string()))?;
        let (result, healthy) = match lease.conn().begin_with("BEGIN IMMEDIATE").await {
            Ok(mut $tx) => match $body.await {
                Ok(value) => match $tx.commit().await {
                    Ok(()) => (Ok(value), true),
                    Err(error) => (Err($sql_error(error)), false),
                },
                Err(error) => {
                    let healthy = $tx.rollback().await.is_ok();
                    (Err(error), healthy)
                }
            },
            Err(error) => (Err($sql_error(error)), false),
        };
        lease.finish(healthy);
        result
    }};
}

/// One read transaction over a reader connection.
macro_rules! read_txn {
    ($store:expr, $tx:ident, $body:expr) => {{
        let mut conn = $store
            .reader()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let mut $tx = conn.begin().await.map_err(store_error)?;
        let value = $body.await;
        let ended = $tx.rollback().await.is_ok();
        if !ended {
            conn.close_on_drop();
        }
        value
    }};
}
