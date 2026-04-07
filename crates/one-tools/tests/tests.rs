use one_tools::{ToolContext, create_default_registry};

#[test]
fn test_registry_has_all_tools() {
    let reg = create_default_registry();
    let names = reg.names();

    assert!(names.contains(&"Read"));
    assert!(names.contains(&"Write"));
    assert!(names.contains(&"Edit"));
    assert!(names.contains(&"Bash"));
    assert!(names.contains(&"Grep"));
    assert!(names.contains(&"Glob"));
    assert!(names.contains(&"web_fetch"));
    assert!(names.contains(&"tool_search"));
    assert!(names.contains(&"ask_user"));
    assert!(names.contains(&"sleep"));
    assert!(names.contains(&"enter_plan_mode"));
    assert!(names.contains(&"exit_plan_mode"));
    assert!(names.contains(&"Agent"));
    assert!(names.contains(&"list_mcp_resources"));
    assert!(names.contains(&"read_mcp_resource"));
    assert!(names.contains(&"cron_create"));
    assert!(names.contains(&"cron_delete"));
    assert!(names.contains(&"cron_list"));
    assert!(names.contains(&"notebook_edit"));
    assert!(names.contains(&"Skill"));
    assert_eq!(names.len(), 24);
}

#[test]
fn test_registry_schemas() {
    let reg = create_default_registry();
    let schemas = reg.schemas();

    assert_eq!(schemas.len(), 24);

    for schema in &schemas {
        assert!(schema["name"].is_string());
        assert!(schema["description"].is_string());
        assert!(schema["input_schema"].is_object());
    }
}

#[test]
fn test_registry_lookup() {
    let reg = create_default_registry();

    assert!(reg.get("Read").is_some());
    assert!(reg.get("nonexistent").is_none());

    let tool = reg.get("Read").unwrap();
    assert_eq!(tool.name(), "Read");
    assert!(tool.is_read_only());
}

#[test]
fn test_read_only_tools() {
    let reg = create_default_registry();

    // Read-only tools
    assert!(reg.get("Read").unwrap().is_read_only());
    assert!(reg.get("Grep").unwrap().is_read_only());
    assert!(reg.get("Glob").unwrap().is_read_only());

    // Write tools are not read-only
    assert!(!reg.get("Write").unwrap().is_read_only());
    assert!(!reg.get("Edit").unwrap().is_read_only());
    assert!(!reg.get("Bash").unwrap().is_read_only());
}

#[tokio::test]
async fn test_file_read_nonexistent() {
    let reg = create_default_registry();
    let tool = reg.get("Read").unwrap();

    let ctx = ToolContext::new("/tmp", "test");

    let result = tool
        .execute(
            serde_json::json!({"file_path": "/tmp/definitely_does_not_exist_12345.txt"}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.output.contains("does not exist"));
}

#[tokio::test]
async fn test_file_edit_missing_file() {
    let reg = create_default_registry();
    let tool = reg.get("Edit").unwrap();

    let ctx = ToolContext::new("/tmp", "test");

    let result = tool
        .execute(
            serde_json::json!({
                "file_path": "/tmp/nonexistent_file_for_edit.txt",
                "old_string": "foo",
                "new_string": "bar"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.output.contains("does not exist"));
}

#[tokio::test]
async fn test_bash_simple_command() {
    let reg = create_default_registry();
    let tool = reg.get("Bash").unwrap();

    let ctx = ToolContext::new("/tmp", "test");

    let result = tool
        .execute(serde_json::json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.output.contains("hello"));
}

#[tokio::test]
async fn test_bash_failing_command() {
    let reg = create_default_registry();
    let tool = reg.get("Bash").unwrap();

    let ctx = ToolContext::new("/tmp", "test");

    let result = tool
        .execute(serde_json::json!({"command": "false"}), &ctx)
        .await
        .unwrap();

    assert!(result.is_error);
}

#[test]
fn test_deferred_tools() {
    let reg = create_default_registry();

    // web_fetch and web_search should be deferred
    assert!(reg.get("web_fetch").unwrap().should_defer());
    assert!(reg.get("web_search").unwrap().should_defer());

    // Core tools should NOT be deferred
    assert!(!reg.get("Read").unwrap().should_defer());
    assert!(!reg.get("Write").unwrap().should_defer());
    assert!(!reg.get("Bash").unwrap().should_defer());
    assert!(!reg.get("Grep").unwrap().should_defer());
    assert!(!reg.get("tool_search").unwrap().should_defer());

    // Deferred names list
    let deferred = reg.deferred_tool_names();
    assert_eq!(deferred.len(), 13);
    assert!(deferred.contains(&"web_fetch"));
    assert!(deferred.contains(&"web_search"));
    assert!(deferred.contains(&"sleep"));
    assert!(deferred.contains(&"enter_plan_mode"));
    assert!(deferred.contains(&"exit_plan_mode"));
    assert!(deferred.contains(&"EnterWorktree"));
    assert!(deferred.contains(&"ExitWorktree"));
}

#[test]
fn test_active_schemas_exclude_deferred() {
    let reg = create_default_registry();

    let all = reg.schemas();
    let active = reg.active_schemas();

    // Active should exclude deferred tools
    assert_eq!(active.len(), all.len() - 13);

    let active_names: Vec<&str> = active.iter().map(|s| s["name"].as_str().unwrap()).collect();

    assert!(active_names.contains(&"Read"));
    assert!(active_names.contains(&"tool_search"));
    assert!(!active_names.contains(&"web_fetch"));
    assert!(!active_names.contains(&"web_search"));
}

#[test]
fn test_destructive_tools() {
    let reg = create_default_registry();

    assert!(reg.get("Bash").unwrap().is_destructive());
    assert!(!reg.get("Read").unwrap().is_destructive());
    assert!(!reg.get("Grep").unwrap().is_destructive());
}

#[tokio::test]
async fn test_edit_requires_read_first() {
    // Create a temp file
    let dir = std::env::temp_dir();
    let path = dir.join("test_edit_safety.txt");
    std::fs::write(&path, "hello world").unwrap();

    let reg = create_default_registry();
    let edit_tool = reg.get("Edit").unwrap();

    // Create context with empty read_files — file hasn't been read
    let ctx = ToolContext::new(dir.to_string_lossy(), "test");

    let result = edit_tool
        .execute(
            serde_json::json!({
                "file_path": path.to_string_lossy(),
                "old_string": "hello",
                "new_string": "goodbye"
            }),
            &ctx,
        )
        .await
        .unwrap();

    // Should fail because file wasn't read first
    assert!(result.is_error);
    assert!(result.output.contains("not been read"));

    // Now read the file
    let read_tool = reg.get("Read").unwrap();
    let read_result = read_tool
        .execute(
            serde_json::json!({"file_path": path.to_string_lossy()}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!read_result.is_error);

    // Now edit should succeed
    let result = edit_tool
        .execute(
            serde_json::json!({
                "file_path": path.to_string_lossy(),
                "old_string": "hello",
                "new_string": "goodbye"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.output.contains("1 line")); // diff output

    // Verify the file was actually changed
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "goodbye world");

    // Cleanup
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_skill_tool_schema() {
    let reg = create_default_registry();
    let tool = reg.get("Skill").unwrap();

    let schema = tool.input_schema();
    let props = schema["properties"].as_object().unwrap();
    assert!(props.contains_key("skill"));
    assert!(props.contains_key("args"));

    let required = schema["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r == "skill"));
}

#[test]
fn test_todo_write_tool_schema() {
    let reg = create_default_registry();
    let tool = reg.get("TodoWrite").unwrap();

    let schema = tool.input_schema();
    let props = schema["properties"].as_object().unwrap();
    assert!(props.contains_key("todos"));

    // todos should be an array type
    assert_eq!(props["todos"]["type"], "array");
}

#[test]
fn test_worktree_tools_are_deferred() {
    let reg = create_default_registry();

    assert!(reg.get("EnterWorktree").unwrap().should_defer());
    assert!(reg.get("ExitWorktree").unwrap().should_defer());

    // They should have search hints
    assert!(reg.get("EnterWorktree").unwrap().search_hint().is_some());
    assert!(reg.get("ExitWorktree").unwrap().search_hint().is_some());
}
