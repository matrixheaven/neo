use super::*;

#[test]
fn from_store_uses_supplied_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = McpOAuthStore::new(dir.path().to_path_buf());
    let service = McpOAuthService::from_store(store.clone());

    assert_eq!(service.store().root(), store.root());
}

#[test]
fn new_uses_credentials_mcp_store_root() {
    let dir = tempfile::tempdir().unwrap();
    let service = McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: Some(dir.path().to_path_buf()),
    });

    assert_eq!(
        service.store().root(),
        dir.path().join("credentials").join("mcp")
    );
}
