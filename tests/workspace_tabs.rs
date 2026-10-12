use lazydb::model::transaction::TransactionMode;
use lazydb::persistence::workspace::{
    PersistedConsole, PersistedProfileWorkspace, PersistedTab, WorkspaceSnapshot,
};
use lazydb::profile::import_connection_url;
use lazydb::{
    action::{Action, Command},
    app::App,
    db::catalog::{
        CatalogCount, CatalogCursor, CatalogEntry, CatalogGroupSummary, CatalogId, CatalogKind,
        CatalogPage, CatalogRequest, CatalogTarget, ObjectGroup, OptionalMetadata, QualifiedName,
    },
    model::relation::RelationTab,
    model::tab::{ConsoleTab, TabKind, WorkspaceTab},
    model::workspace::Focus,
};
use uuid::Uuid;

#[test]
fn workspace_tabs_expose_common_identity() {
    let console = ConsoleTab::new("sql");
    let console_id = console.id;
    let relation = RelationTab::new("users");
    let relation_id = relation.id;

    let tabs = [WorkspaceTab::Sql(console), WorkspaceTab::Relation(relation)];
    assert_eq!(tabs[0].id(), console_id);
    assert_eq!(tabs[0].title(), "sql");
    assert_eq!(tabs[0].kind(), TabKind::Sql);
    assert_eq!(tabs[1].id(), relation_id);
    assert_eq!(tabs[1].title(), "users");
    assert_eq!(tabs[1].kind(), TabKind::Relation);

    let dashboard = WorkspaceTab::Dashboard(lazydb::model::dashboard::DashboardTab::new());
    assert_eq!(dashboard.title(), "Dashboard");
    assert_eq!(dashboard.kind(), TabKind::Dashboard);

    let redis = WorkspaceTab::RedisBrowser(lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(3),
        lazydb::db::redis::types::RedisTarget {
            profile_id: Uuid::from_u128(4),
            database: 2,
        },
    ));
    assert_eq!(redis.id(), Uuid::from_u128(3));
    assert_eq!(redis.title(), "Redis");
    assert_eq!(redis.kind(), TabKind::RedisBrowser);
}

#[test]
fn moving_tabs_preserves_active_identity_and_focus() {
    let mut app = App::new(Vec::new());
    app.tabs = vec![
        WorkspaceTab::Sql(ConsoleTab::new("first")),
        WorkspaceTab::Sql(ConsoleTab::new("second")),
        WorkspaceTab::Sql(ConsoleTab::new("third")),
    ];
    app.active_tab = 1;
    app.focus = Focus::Results;
    let active_id = app.tabs[app.active_tab].id();
    let first_id = app.tabs[0].id();
    let third_id = app.tabs[2].id();

    assert!(app.update(Action::MoveTabRight).is_empty());
    assert_eq!(app.tabs[app.active_tab].id(), active_id);
    assert_eq!(
        app.tabs.iter().map(WorkspaceTab::id).collect::<Vec<_>>(),
        vec![first_id, third_id, active_id]
    );
    assert_eq!(app.focus, Focus::Results);

    assert!(app.update(Action::MoveTabLeft).is_empty());
    assert_eq!(app.tabs[app.active_tab].id(), active_id);
    assert_eq!(app.active_tab, 1);
    assert_eq!(app.focus, Focus::Results);
}

#[test]
fn moving_tabs_at_edges_is_a_noop() {
    let mut app = App::new(Vec::new());
    app.tabs = vec![
        WorkspaceTab::Sql(ConsoleTab::new("first")),
        WorkspaceTab::Sql(ConsoleTab::new("second")),
    ];
    let ids = app.tabs.iter().map(WorkspaceTab::id).collect::<Vec<_>>();

    app.active_tab = 0;
    assert!(app.update(Action::MoveTabLeft).is_empty());
    assert_eq!(
        app.tabs.iter().map(WorkspaceTab::id).collect::<Vec<_>>(),
        ids
    );

    app.active_tab = 1;
    assert!(app.update(Action::MoveTabRight).is_empty());
    assert_eq!(
        app.tabs.iter().map(WorkspaceTab::id).collect::<Vec<_>>(),
        ids
    );
}

#[test]
fn closing_a_restored_offline_tab_does_not_resurrect_after_snapshot() {
    use lazydb::persistence::workspace::WorkspaceStore;

    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        active_profile: Some(first.id),
        profiles: vec![
            PersistedProfileWorkspace {
                profile_id: first.id,
                active_tab: None,
                consoles: Vec::new(),
                tabs: Vec::new(),
            },
            PersistedProfileWorkspace {
                profile_id: second.id,
                active_tab: Some(console_id),
                consoles: vec![persisted_console(
                    console_id,
                    "second",
                    true,
                    Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&second)),
                )],
                tabs: vec![PersistedTab::Console { console_id }],
            },
        ],
        active_console: Uuid::nil(),
        consoles: Vec::new(),
        tabs: Vec::new(),
        sql: vec![(console_id, "select 42".into())],
        recent_targets: Vec::new(),
    };
    let mut app = App::new(vec![first, second]);
    app.restore_workspace(snapshot, None);

    assert!(app.tabs.iter().any(|tab| tab.id() == console_id));
    app.update(Action::CloseTab(console_id));

    assert!(app.tabs.iter().all(|tab| tab.id() != console_id));
    let saved = app.workspace_snapshot();
    assert!(saved.profiles.iter().all(|workspace| {
        workspace.tabs.iter().all(
            |tab| !matches!(tab, PersistedTab::Console { console_id: id } if *id == console_id),
        )
    }));

    let temp = tempfile::TempDir::new().unwrap();
    let store = WorkspaceStore::new(temp.path().join("workspace.toml"), temp.path().join("sql"));
    store.save(&saved).unwrap();
    let mut restarted = App::new(Vec::new());
    restarted.restore_workspace(store.load().unwrap().unwrap(), None);
    assert!(restarted.tabs.iter().all(|tab| tab.id() != console_id));
}

#[test]
fn restored_active_console_stays_offline_without_explicit_startup_selection() {
    let profile = import_connection_url(":memory:", Some("restored"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(console_id),
            consoles: vec![PersistedConsole {
                id: console_id,
                name: "restored".into(),
                sql_file: format!("{console_id}.sql").into(),
                target: Some(
                    lazydb::model::execution_target::ExecutionTarget::from_profile(&profile),
                ),
                transaction_mode: TransactionMode::Auto,
                open: true,
            }],
            tabs: vec![PersistedTab::Console { console_id }],
        }],
        active_console: Uuid::nil(),
        consoles: Vec::new(),
        tabs: Vec::new(),
        sql: vec![(console_id, "select 42".into())],
        recent_targets: Vec::new(),
    };
    let mut app = App::new(vec![profile.clone()]);
    app.restore_workspace(snapshot, None);
    app.focus = Focus::Explorer;

    lazydb::runtime::apply_startup_action(&mut app, None);

    assert_eq!(app.active_console().id, console_id);
    assert_eq!(
        app.connection.status,
        lazydb::model::workspace::ConnectionStatus::Disconnected
    );
    assert!(app.connection.pending_target.is_none());
    assert_eq!(app.focus, Focus::Explorer);
    assert_eq!(app.active_editor_text().unwrap(), "select 42");
}

#[test]
fn restored_active_console_stays_offline_when_editor_focus_is_entered() {
    let profile = import_connection_url(":memory:", Some("restored"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let target = lazydb::model::execution_target::ExecutionTarget::from_profile(&profile);
    let snapshot = WorkspaceSnapshot {
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(console_id),
            consoles: vec![PersistedConsole {
                id: console_id,
                name: "restored".into(),
                sql_file: format!("{console_id}.sql").into(),
                target: Some(target.clone()),
                transaction_mode: TransactionMode::Auto,
                open: true,
            }],
            tabs: vec![PersistedTab::Console { console_id }],
        }],
        active_console: Uuid::nil(),
        consoles: Vec::new(),
        tabs: Vec::new(),
        sql: vec![(console_id, "select 42".into())],
        recent_targets: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);
    app.focus = Focus::Explorer;

    let commands = app.update(Action::Focus(Focus::Editor));

    assert_eq!(app.focus, Focus::Editor);
    assert!(app.connection.pending_target.is_none());
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert_eq!(
        app.active_console().execution_target.as_ref(),
        Some(&target)
    );
}

fn restored_console_for_focus_test() -> (App, lazydb::model::execution_target::ExecutionTarget) {
    let profile = import_connection_url(":memory:", Some("restored"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let target = lazydb::model::execution_target::ExecutionTarget::from_profile(&profile);
    let snapshot = WorkspaceSnapshot {
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(console_id),
            consoles: vec![PersistedConsole {
                id: console_id,
                name: "restored".into(),
                sql_file: format!("{console_id}.sql").into(),
                target: Some(target.clone()),
                transaction_mode: TransactionMode::Auto,
                open: true,
            }],
            tabs: vec![PersistedTab::Console { console_id }],
        }],
        active_console: Uuid::nil(),
        consoles: Vec::new(),
        tabs: Vec::new(),
        sql: vec![(console_id, "select 42".into())],
        recent_targets: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);
    app.focus = Focus::Explorer;
    (app, target)
}

#[test]
fn restored_active_console_stays_offline_when_results_focus_is_entered() {
    let (mut app, target) = restored_console_for_focus_test();

    let commands = app.update(Action::Focus(Focus::Results));

    assert_eq!(app.focus, Focus::Results);
    assert!(app.connection.pending_target.is_none());
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert_eq!(
        app.active_console().execution_target.as_ref(),
        Some(&target)
    );
}

#[test]
fn focus_cycle_enters_restored_console_without_connecting() {
    for action in [Action::FocusNext, Action::FocusPrevious] {
        let (mut app, target) = restored_console_for_focus_test();

        let commands = app.update(action);

        assert!(matches!(app.focus, Focus::Editor | Focus::Results));
        assert!(app.connection.pending_target.is_none());
        assert!(
            commands
                .iter()
                .all(|command| !matches!(command, Command::Connect { .. }))
        );
        assert_eq!(
            app.active_console().execution_target.as_ref(),
            Some(&target)
        );
    }
}

#[test]
fn activating_restored_console_tab_enters_editor_without_connecting() {
    let (mut app, target) = restored_console_for_focus_test();

    let commands = app.update(Action::ActivateTab(0));

    assert_eq!(app.focus, Focus::Editor);
    assert!(app.connection.pending_target.is_none());
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert_eq!(
        app.active_console().execution_target.as_ref(),
        Some(&target)
    );
}

#[test]
fn console_tab_title_remains_the_persisted_name() {
    let console = ConsoleTab::new("analysis");
    let tab = WorkspaceTab::Sql(console);

    assert_eq!(tab.title(), "analysis");
}

#[test]
fn global_console_manager_search_uses_profile_target_and_pure_name_order() {
    let profile = import_connection_url(":memory:", Some("warehouse"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![profile.clone()]);
    let open_id = Uuid::from_u128(2);
    let mut open_tab = ConsoleTab::new("zeta");
    open_tab.id = open_id;
    app.tabs.push(WorkspaceTab::Sql(open_tab));
    app.sql_editors = vec![
        lazydb::model::tab::ConsoleRecord {
            id: open_id,
            name: "zeta".into(),
            execution_target: Some(lazydb::model::execution_target::ExecutionTarget {
                profile_id: profile.id,
                database: "analytics_db".into(),
                schema: Some("reporting".into()),
            }),
            transaction_mode: TransactionMode::Auto,
            open: true,
        },
        lazydb::model::tab::ConsoleRecord {
            id: Uuid::from_u128(1),
            name: "alpha".into(),
            execution_target: None,
            transaction_mode: TransactionMode::Auto,
            open: false,
        },
    ];

    assert_eq!(
        app.visible_console_ids(""),
        [Uuid::from_u128(2), Uuid::from_u128(1)]
    );
    for query in ["warehouse", "analytics_db", "REPORTING"] {
        assert_eq!(
            app.visible_console_ids(query),
            [Uuid::from_u128(2)],
            "{query}"
        );
    }
}

#[test]
fn opening_dashboard_is_idempotent_and_focuses_results() {
    let mut app = App::new(Vec::new());
    app.update(Action::OpenDashboard);
    app.update(Action::OpenDashboard);

    assert_eq!(
        app.tabs
            .iter()
            .filter(|tab| tab.kind() == TabKind::Dashboard)
            .count(),
        1
    );
    assert_eq!(app.focus, Focus::Results);
    assert_eq!(app.tabs[app.active_tab].title(), "Dashboard");
}

#[test]
fn dashboard_cycles_focus_only_between_explorer_and_results() {
    let mut app = App::new(Vec::new());
    app.tabs.clear();
    app.tabs.push(WorkspaceTab::Dashboard(
        lazydb::model::dashboard::DashboardTab::new(),
    ));
    app.active_tab = 0;
    app.focus = Focus::Explorer;

    app.update(Action::FocusNext);
    assert_eq!(app.focus, Focus::Results);
    app.update(Action::FocusNext);
    assert_eq!(app.focus, Focus::Explorer);
    app.update(Action::FocusPrevious);
    assert_eq!(app.focus, Focus::Results);
}

#[test]
fn empty_workspace_keeps_focus_in_explorer() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    app.active_workspace_profile = Some(profile_id);
    assert!(app.tabs.is_empty());

    for action in [
        Action::FocusNext,
        Action::FocusPrevious,
        Action::Focus(Focus::Editor),
        Action::Focus(Focus::Results),
        Action::Focus(Focus::Explorer),
    ] {
        app.focus = Focus::Explorer;
        assert!(app.update(action).is_empty());
        assert_eq!(app.focus, Focus::Explorer);
    }
}

#[test]
fn new_app_with_profiles_has_no_active_workspace_until_connected() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let app = App::new(vec![profile]);

    assert_eq!(app.active_workspace_profile, None);
    assert!(app.tabs.is_empty());
    assert!(app.sql_editors.is_empty());
    assert!(app.active_console_opt().is_none());
}

#[test]
fn connection_workspace_preserves_mixed_tab_order() {
    let console = ConsoleTab::new("sql");
    let relation = RelationTab::new("users");
    let console_id = console.id;
    let relation_id = relation.id;
    let workspace = lazydb::model::workspace::ConnectionWorkspace {
        tabs: vec![WorkspaceTab::Relation(relation), WorkspaceTab::Sql(console)],
        sql_editors: Vec::new(),
        sql: Vec::new(),
        active_tab_id: Some(console_id),
    };

    assert_eq!(workspace.tabs[0].id(), relation_id);
    assert_eq!(workspace.tabs[1].id(), console_id);
    assert_eq!(workspace.active_tab_id, Some(console_id));
}

#[test]
fn connection_workspace_active_tab_is_an_id_not_an_index() {
    let first = ConsoleTab::new("first");
    let second = ConsoleTab::new("second");
    let second_id = second.id;
    let workspace = lazydb::model::workspace::ConnectionWorkspace {
        tabs: vec![WorkspaceTab::Sql(first), WorkspaceTab::Sql(second)],
        sql_editors: Vec::new(),
        sql: Vec::new(),
        active_tab_id: Some(second_id),
    };

    let reordered = [workspace.tabs[1].id(), workspace.tabs[0].id()];
    assert_eq!(reordered[0], workspace.active_tab_id.unwrap());
}

#[test]
fn relation_tabs_have_no_console_accessor() {
    let mut tab = WorkspaceTab::Relation(RelationTab::new("users"));
    assert!(tab.as_console().is_none());
    assert!(tab.as_console_mut().is_none());
}

#[test]
fn mixed_tabs_cycle_and_activate_without_sql_assumptions() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));

    app.update(Action::NextTab);
    assert_eq!(app.active_tab, 1);
    app.update(Action::NextTab);
    assert_eq!(app.active_tab, 0);
    app.update(Action::PreviousTab);
    assert_eq!(app.active_tab, 1);
}

#[test]
fn cycling_relation_tabs_normalizes_editor_focus() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));
    app.focus = Focus::Editor;

    app.update(Action::NextTab);

    assert_eq!(app.active_tab, 1);
    assert_eq!(app.focus, Focus::Results);

    app.focus = Focus::Editor;
    app.update(Action::PreviousTab);
    assert_eq!(app.active_tab, 0);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn cycling_console_tabs_focuses_sql_editor() {
    let mut app = App::new(Vec::new());
    app.update(Action::NewConsole);
    app.update(Action::NewConsole);
    app.focus = Focus::Results;

    app.update(Action::NextTab);

    assert_eq!(app.focus, Focus::Editor);
    app.focus = Focus::Results;
    app.update(Action::PreviousTab);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn closing_relation_tab_bypasses_transaction_exit() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));
    app.active_tab = 1;

    let commands = app.update(Action::CloseActiveTab);

    assert!(
        commands.is_empty()
            || commands.iter().any(|command| {
                matches!(command, lazydb::action::Command::PersistWorkspace { .. })
            })
    );
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn activating_relation_normalizes_editor_focus() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));
    app.focus = Focus::Editor;

    app.update(Action::ActivateTab(1));

    assert_eq!(app.active_tab, 1);
    assert_ne!(app.focus, Focus::Editor);
}

#[test]
fn closing_final_sql_console_leaves_workspace_empty() {
    let mut app = App::new(Vec::new());
    let commands = app.update(Action::CloseActiveTab);

    assert!(app.tabs.is_empty());
    assert!(app.active_console_opt().is_none());
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::PersistWorkspace { .. }))
    );
}

#[test]
fn final_console_can_be_closed_and_deleted() {
    let mut app = App::new(Vec::new());
    let id = app.active_console().id;

    app.update(Action::RequestDeleteActiveConsole);
    app.update(Action::ToggleDeleteConsoleFocus);
    let commands = app.update(Action::ConfirmDeleteConsole);
    assert!(!app.sql_editors.iter().any(|record| record.id == id));
    assert!(app.tabs.is_empty());
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::PersistWorkspace { .. }))
    );
    let revision = commands
        .iter()
        .find_map(|command| match command {
            Command::PersistWorkspace { revision, .. } => Some(*revision),
            _ => None,
        })
        .unwrap();
    let commands = app.update(Action::WorkspaceSaveSucceeded { revision });
    assert!(
        commands.iter().any(
            |command| matches!(command, Command::DeleteSqlFile(console_id) if *console_id == id)
        )
    );
}

#[test]
fn non_default_console_can_still_be_closed_and_deleted() {
    let mut app = App::new(Vec::new());
    app.update(Action::NewConsole);
    let id = app.active_console().id;

    app.update(Action::CloseActiveTab);
    assert!(app.tabs.iter().all(|tab| tab.id() != id));
    assert!(
        app.sql_editors
            .iter()
            .any(|record| record.id == id && !record.open)
    );

    app.update(Action::ActivateSqlEditor(id));
    app.update(Action::RequestDeleteActiveConsole);
    assert!(matches!(
        app.overlay,
        Some(lazydb::model::workspace::Overlay::DeleteConsole { console_id, .. })
            if console_id == id
    ));

    app.update(Action::ToggleDeleteConsoleFocus);
    let commands = app.update(Action::ConfirmDeleteConsole);
    assert!(!app.sql_editors.iter().any(|record| record.id == id));
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::PersistWorkspace { .. }))
    );
    let revision = commands
        .iter()
        .find_map(|command| match command {
            Command::PersistWorkspace { revision, .. } => Some(*revision),
            _ => None,
        })
        .unwrap();
    let commands = app.update(Action::WorkspaceSaveSucceeded { revision });
    assert!(
        commands.iter().any(
            |command| matches!(command, Command::DeleteSqlFile(console_id) if *console_id == id)
        )
    );
}

#[test]
fn closing_other_tabs_keeps_only_the_active_console() {
    let mut app = App::new(Vec::new());
    let _default_id = app.active_console().id;
    app.update(Action::NewConsole);
    let second_id = app.active_console().id;

    let commands = app.update(Action::CloseOtherTabs);

    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::PersistWorkspace { .. }))
    );
    assert!(app.tabs.iter().all(|tab| tab.id() == second_id));
    assert_eq!(app.active_console().id, second_id);
}

#[test]
fn closing_other_tabs_from_a_console_removes_the_other_tabs() {
    let mut app = App::new(Vec::new());
    let default_id = app.active_console().id;
    app.update(Action::NewConsole);
    let second_id = app.active_console().id;
    app.update(Action::ActivateSqlEditor(default_id));

    app.update(Action::CloseOtherTabs);

    assert_eq!(
        app.tabs.iter().map(|tab| tab.id()).collect::<Vec<_>>(),
        vec![default_id]
    );
    assert!(
        !app.sql_editors
            .iter()
            .find(|record| record.id == second_id)
            .unwrap()
            .open
    );
    assert_eq!(app.active_console().id, default_id);
}

#[test]
fn closing_and_reopening_sql_editor_preserves_persisted_text_and_target() {
    let mut app = App::new(Vec::new());
    app.update(Action::NewConsole);
    let editor_id = app.active_console().id;
    app.update(Action::ReplaceEditor("select 42".into()));
    app.update(Action::CloseActiveTab);

    assert!(
        !app.sql_editors
            .iter()
            .find(|record| record.id == editor_id)
            .unwrap()
            .open
    );
    assert!(app.tabs.iter().all(|tab| tab.id() != editor_id));
    assert_eq!(app.editor_text(editor_id).unwrap(), "select 42");

    app.update(Action::ActivateSqlEditor(editor_id));

    assert_eq!(app.active_console().id, editor_id);
    assert_eq!(app.active_editor_text().unwrap(), "select 42");
    assert!(app.active_console().execution_target.is_none());
}

#[test]
fn workspace_snapshot_restores_open_and_closed_consoles_with_sql_and_names() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![profile.clone()]);
    app.connection.profile_id = Some(profile.id);
    app.update(Action::NewConsole);
    let open_id = app.active_console().id;
    app.update(Action::ReplaceEditor("select open;".into()));
    app.update(Action::OpenSqlEditorList);
    app.update(Action::SqlEditorListRenameStart);
    app.update(Action::SqlEditorListInputDeleteToStart);
    for character in "renamed open".chars() {
        app.update(Action::SqlEditorListInputInsert(character));
    }
    app.update(Action::SqlEditorListRenameCommit);

    app.update(Action::NewConsole);
    let closed_id = app.active_console().id;
    app.update(Action::ReplaceEditor("select closed;".into()));
    app.update(Action::CloseActiveTab);
    app.update(Action::OpenSqlEditorList);
    if let Some(lazydb::model::workspace::Overlay::SqlEditorList(list)) = app.overlay.as_mut() {
        list.selected_id = Some(closed_id);
    }
    app.update(Action::SqlEditorListRenameStart);
    app.update(Action::SqlEditorListInputDeleteToStart);
    for character in "renamed closed".chars() {
        app.update(Action::SqlEditorListInputInsert(character));
    }
    app.update(Action::SqlEditorListRenameCommit);

    let snapshot = app.workspace_snapshot();
    let mut restored = App::new(vec![profile.clone()]);
    restored.connection.profile_id = Some(profile.id);
    restored.restore_workspace(snapshot, Some(profile.id));

    assert_eq!(restored.active_console().id, open_id);
    assert_eq!(restored.active_console().name, "renamed open");
    assert_eq!(restored.editor_text(open_id).unwrap(), "select open;");
    let closed = restored
        .sql_editors
        .iter()
        .find(|record| record.id == closed_id)
        .unwrap();
    assert_eq!(closed.name, "renamed closed");
    assert!(!closed.open);
    assert_eq!(restored.editor_text(closed_id).unwrap(), "select closed;");
    assert!(restored.tabs.iter().all(|tab| tab.id() != closed_id));

    restored.update(Action::ActivateSqlEditor(closed_id));
    assert_eq!(restored.active_console().id, closed_id);
    assert_eq!(restored.active_console().name, "renamed closed");
    assert_eq!(restored.active_editor_text().unwrap(), "select closed;");
}

#[test]
fn workspace_restore_keeps_a_closed_console_closed_with_its_original_identity() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let default_id = Uuid::from_u128(101);
    let other_id = Uuid::from_u128(102);
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(other_id),
            consoles: vec![
                PersistedConsole {
                    id: default_id,
                    name: "renamed default".into(),
                    sql_file: format!("{default_id}.sql").into(),
                    target: None,
                    transaction_mode: TransactionMode::Auto,
                    open: false,
                },
                PersistedConsole {
                    id: other_id,
                    name: "other".into(),
                    sql_file: format!("{other_id}.sql").into(),
                    target: None,
                    transaction_mode: TransactionMode::Auto,
                    open: true,
                },
            ],
            tabs: vec![PersistedTab::Console {
                console_id: other_id,
            }],
        }],
        sql: vec![
            (default_id, "select default;".into()),
            (other_id, "select other;".into()),
        ],
        active_console: other_id,
        tabs: Vec::new(),
        consoles: Vec::new(),
    };
    let mut app = App::new(vec![profile.clone()]);
    app.connection.profile_id = Some(profile.id);
    app.restore_workspace(snapshot, Some(profile.id));

    assert!(!app.tabs.iter().any(|tab| tab.id() == default_id));
    assert!(
        !app.sql_editors
            .iter()
            .find(|record| record.id == default_id)
            .unwrap()
            .open
    );
    assert_eq!(app.editor_text(default_id).unwrap(), "select default;");
}

#[test]
fn deleting_sql_editor_requires_confirmation_and_removes_record() {
    let mut app = App::new(Vec::new());
    app.update(Action::NewConsole);
    let editor_id = app.active_console().id;

    assert!(app.update(Action::RequestDeleteActiveConsole).is_empty());
    assert!(matches!(
        app.overlay,
        Some(lazydb::model::workspace::Overlay::DeleteConsole { console_id, .. })
            if console_id == editor_id
    ));

    app.update(Action::CancelDeleteConsole);
    assert!(app.sql_editors.iter().any(|record| record.id == editor_id));

    app.update(Action::RequestDeleteActiveConsole);
    app.update(Action::ToggleDeleteConsoleFocus);
    app.update(Action::ConfirmDeleteConsole);
    assert!(!app.sql_editors.iter().any(|record| record.id == editor_id));
    assert!(app.tabs.iter().all(|tab| tab.id() != editor_id));
    assert!(app.active_console_opt().is_some());
}

#[test]
fn sql_editor_list_reopens_a_hidden_editor() {
    let mut app = App::new(Vec::new());
    app.update(Action::NewConsole);
    let editor_id = app.active_console().id;
    app.update(Action::ReplaceEditor("select 7".into()));
    app.update(Action::CloseActiveTab);

    app.update(Action::OpenSqlEditorList);
    app.update(Action::ActivateSqlEditor(editor_id));

    assert_eq!(app.active_console().id, editor_id);
    assert_eq!(app.active_editor_text().unwrap(), "select 7");
}

#[test]
fn workspace_restore_keeps_hidden_editors_hidden() {
    let default_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: None,
        profiles: Vec::new(),
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: vec![
            PersistedConsole {
                id: default_id,
                name: "default".into(),
                sql_file: format!("{default_id}.sql").into(),
                target: None,
                transaction_mode: TransactionMode::Auto,
                open: false,
            },
            PersistedConsole {
                id,
                name: "closed".into(),
                sql_file: format!("{id}.sql").into(),
                target: None,
                transaction_mode: TransactionMode::Auto,
                open: false,
            },
        ],
        sql: vec![
            (default_id, "select default".into()),
            (id, "select 9".into()),
        ],
    };
    let mut app = App::new(Vec::new());
    app.restore_workspace(snapshot, None);

    assert!(
        app.sql_editors
            .iter()
            .any(|record| record.id == id && !record.open)
    );
    assert!(app.tabs.is_empty());
    assert!(app.tabs.iter().all(|tab| tab.id() != id));
}

#[test]
fn sql_only_actions_are_noops_on_relation_tabs() {
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::Relation(RelationTab::new("users")));
    app.active_tab = 1;

    for action in [
        Action::CancelActiveQuery,
        Action::SetTransactionMode(lazydb::model::transaction::TransactionMode::Manual),
        Action::CommitTransaction,
        Action::RollbackTransaction,
        Action::ClearTransactionOutcome,
        Action::GridMove {
            rows: 1,
            columns: 1,
        },
        Action::GridSelect { row: 1, column: 1 },
        Action::ToggleResultView,
        Action::CompletionExplicit,
        Action::CompletionNext,
        Action::CompletionPrevious,
        Action::CompletionAccept,
        Action::CompletionDismiss,
        Action::RunActiveSql,
        Action::RunAllSql,
        Action::ReplaceEditor("select 1".to_owned()),
    ] {
        assert!(app.update(action).is_empty());
    }
}

#[test]
fn initial_and_new_consoles_use_the_active_profile_target() {
    let profile = import_connection_url(":memory:", Some("active"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![profile.clone()]);
    assert!(app.active_console_opt().is_none());

    app.connection.profile_id = Some(profile.id);
    let expected = lazydb::model::execution_target::ExecutionTarget::from_profile(&profile);
    app.update(Action::NewConsole);
    assert_eq!(
        app.active_console().execution_target.as_ref(),
        Some(&expected)
    );
}

#[test]
fn explicit_console_target_binding_updates_console_without_connecting() {
    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.connection.profile_id = Some(first.id);
    app.update(Action::NewConsole);
    let console_id = app.active_console().id;
    app.update(Action::ReplaceEditor("select 1".into()));
    let target = lazydb::model::execution_target::ExecutionTarget::from_profile(&second);

    app.update(Action::OpenConsoleTargetSelector { console_id });
    let selected = match app.overlay.as_ref().unwrap() {
        lazydb::model::workspace::Overlay::TargetSelector { candidates, .. } => candidates
            .iter()
            .position(|candidate| candidate.target() == Some(&target))
            .unwrap(),
        overlay => panic!("unexpected overlay: {overlay:?}"),
    };
    let commands = app.update(Action::SelectTargetSelector(selected));

    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::PersistWorkspace { .. }))
    );
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert_eq!(app.active_console().execution_target, Some(target.clone()));
    assert_eq!(
        app.sql_editors
            .iter()
            .find(|record| record.id == console_id)
            .unwrap()
            .execution_target,
        Some(target)
    );
    assert_eq!(app.active_editor_text().unwrap(), "select 1");
    assert!(app.overlay.is_none());
}

#[test]
fn explicit_console_target_binding_respects_active_transaction() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![profile.clone()]);
    app.connection.profile_id = Some(profile.id);
    app.update(Action::NewConsole);
    let console_id = app.active_console().id;
    app.active_console_mut().transaction_state =
        lazydb::model::transaction::TransactionState::Active;
    app.update(Action::OpenConsoleTargetSelector { console_id });
    let commands = app.update(Action::ConfirmTargetSelector);
    assert!(commands.is_empty());
    assert!(
        app.notifications
            .history()
            .any(|notification| notification.body.contains("active transaction"))
    );
}

#[test]
fn new_console_uses_the_profile_default_instead_of_connected_target() {
    let mut profile = import_connection_url(":memory:", Some("active"))
        .unwrap()
        .profile;
    profile.catalog_scope.databases = lazydb::profile::CatalogSelection::All;
    let mut app = App::new(vec![profile.clone()]);
    let connected_target = lazydb::model::execution_target::ExecutionTarget {
        profile_id: profile.id,
        database: ":memory:".into(),
        schema: Some("attached".into()),
    };
    app.connection.profile_id = Some(profile.id);
    app.connection.target = Some(connected_target.clone());
    app.connection.status = lazydb::model::workspace::ConnectionStatus::Connected;
    app.update(Action::NewConsole);

    assert_ne!(
        app.active_console().execution_target.as_ref(),
        Some(&connected_target)
    );
    assert_eq!(
        app.active_console()
            .execution_target
            .as_ref()
            .and_then(|target| target.schema.as_deref()),
        Some("main")
    );
}

#[test]
fn workspace_restore_preserves_valid_targets_and_unbound_documents() {
    let profile = import_connection_url(":memory:", Some("active"))
        .unwrap()
        .profile;
    let expected = lazydb::model::execution_target::ExecutionTarget::from_profile(&profile);
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile.id),
        profiles: Vec::new(),
        active_console: second,
        tabs: Vec::new(),
        consoles: vec![
            PersistedConsole {
                id: first,
                name: "saved".into(),
                sql_file: format!("{first}.sql").into(),
                target: Some(expected.clone()),
                transaction_mode: TransactionMode::Auto,
                open: true,
            },
            PersistedConsole {
                id: second,
                name: "missing".into(),
                sql_file: format!("{second}.sql").into(),
                target: None,
                transaction_mode: TransactionMode::Manual,
                open: true,
            },
        ],
        sql: vec![(first, "select 1".into()), (second, "select 2".into())],
    };
    let mut app = App::new(vec![profile.clone()]);
    app.restore_workspace(snapshot, Some(profile.id));

    assert_eq!(app.active_console().id, second);
    assert_eq!(app.active_console().execution_target.as_ref(), None);
    assert_eq!(
        app.active_console().transaction_mode,
        TransactionMode::Manual
    );
    assert_eq!(app.active_editor_text().unwrap(), "select 2");
    assert_eq!(
        app.tabs[0].as_console().unwrap().execution_target.as_ref(),
        Some(&expected)
    );
}

#[test]
fn workspace_restore_exposes_all_saved_console_documents_before_connecting() {
    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(second.id),
        profiles: vec![
            profile_workspace(
                first.id,
                first_id,
                None,
                Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&first)),
            ),
            profile_workspace(
                second.id,
                second_id,
                Some(second_id),
                Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&second)),
            ),
        ],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![
            (first_id, "select first".into()),
            (second_id, "select second".into()),
        ],
    };
    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.restore_workspace(snapshot, Some(first.id));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.sql_editors.len(), 2);
    assert_eq!(app.active_workspace_profile, Some(first.id));
    let restored = app.workspace_snapshot();
    assert_eq!(
        restored
            .profiles
            .iter()
            .map(|profile| profile.profile_id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert_eq!(restored.sql.len(), 2);
}

#[test]
fn workspace_restore_ignores_invalid_or_deleted_targets_and_uses_first_profile() {
    let profile = import_connection_url(":memory:", Some("startup"))
        .unwrap()
        .profile;
    let invalid_id = Uuid::new_v4();
    let deleted_profile = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: None,
        profiles: vec![profile_workspace(
            deleted_profile,
            invalid_id,
            Some(invalid_id),
            None,
        )],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![(invalid_id, "select 1".into())],
    };
    let mut app = App::new(vec![profile.clone()]);
    app.restore_workspace(snapshot, None);

    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.sql_editors.len(), 1);
    assert_eq!(app.active_workspace_profile, None);
}

#[test]
fn workspace_restore_assigns_targetless_consoles_to_startup_then_first_profile() {
    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: None,
        profiles: vec![profile_workspace(Uuid::nil(), id, Some(id), None)],
        active_console: id,
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![(id, "select 1".into())],
    };
    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.restore_workspace(snapshot.clone(), Some(second.id));
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.sql_editors.len(), 1);

    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.restore_workspace(snapshot, None);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.sql_editors.len(), 2);
    assert!(app.sql_editors.iter().any(|record| record.id == id));
}

#[test]
fn workspace_restore_rebuilds_all_profile_tabs_and_preserves_hidden_sql() {
    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let hidden_id = Uuid::new_v4();
    let relation_id = Uuid::new_v4();
    let other_console_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(first.id),
        profiles: vec![
            PersistedProfileWorkspace {
                profile_id: first.id,
                active_tab: Some(relation_id),
                consoles: vec![
                    persisted_console(
                        console_id,
                        "first",
                        true,
                        Some(
                            lazydb::model::execution_target::ExecutionTarget::from_profile(&first),
                        ),
                    ),
                    persisted_console(hidden_id, "hidden", false, None),
                ],
                tabs: vec![
                    PersistedTab::Console { console_id },
                    PersistedTab::Relation(lazydb::persistence::workspace::PersistedRelationTab {
                        id: relation_id,
                        object_id: lazydb::db::catalog::CatalogId::new(
                            first.id,
                            lazydb::db::catalog::CatalogKind::Table,
                            ["users"],
                        ),
                        qualified_name: lazydb::db::catalog::QualifiedName {
                            database: None,
                            schema: Some("public".into()),
                            object: "users".into(),
                        },
                        catalog_kind: lazydb::db::catalog::CatalogKind::Table,
                        title: "users".into(),
                        view: lazydb::model::relation::RelationView::Ddl,
                    }),
                ],
            },
            profile_workspace(
                second.id,
                other_console_id,
                Some(other_console_id),
                Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&second)),
            ),
        ],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![
            (console_id, "select first".into()),
            (hidden_id, "select hidden".into()),
            (other_console_id, "select second".into()),
        ],
    };
    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.restore_workspace(snapshot.clone(), Some(first.id));

    assert_eq!(app.tabs.len(), 3);
    assert_eq!(app.sql_editors.len(), 3);
    assert_eq!(app.active_workspace_profile, Some(first.id));

    let restored = app.workspace_snapshot();
    assert_eq!(restored.profiles.len(), 2);
    assert_eq!(
        restored
            .sql
            .iter()
            .map(|(_, text)| text)
            .collect::<std::collections::HashSet<_>>(),
        snapshot
            .sql
            .iter()
            .map(|(_, text)| text)
            .collect::<std::collections::HashSet<_>>()
    );
    let first_workspace = restored
        .profiles
        .iter()
        .find(|profile| profile.profile_id == first.id)
        .unwrap();
    assert!(first_workspace.tabs.iter().any(|tab| matches!(
        tab,
        PersistedTab::Console { console_id: id } if *id == console_id
    )));
    assert!(first_workspace.tabs.iter().any(|tab| matches!(
        tab,
        PersistedTab::Relation(relation) if relation.id == relation_id
    )));
}

#[test]
fn closing_restored_tabs_without_connecting_survives_disk_round_trip() {
    use lazydb::persistence::workspace::{PersistedRelationTab, WorkspaceStore};

    for missing_profile in [false, true] {
        let first = import_connection_url(":memory:", Some("first"))
            .unwrap()
            .profile;
        let second = import_connection_url("redis://localhost:6379", Some("second"))
            .unwrap()
            .profile;
        let console_id = Uuid::new_v4();
        let relation_id = Uuid::new_v4();
        let dashboard_id = Uuid::new_v4();
        let redis_id = Uuid::new_v4();
        let mut console = persisted_console(console_id, "saved SQL", true, None);
        console.target =
            Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&second));
        let snapshot = WorkspaceSnapshot {
            active_profile: Some(first.id),
            profiles: vec![
                PersistedProfileWorkspace {
                    profile_id: first.id,
                    active_tab: None,
                    consoles: Vec::new(),
                    tabs: Vec::new(),
                },
                PersistedProfileWorkspace {
                    profile_id: second.id,
                    active_tab: Some(relation_id),
                    consoles: vec![console],
                    tabs: vec![
                        PersistedTab::Console { console_id },
                        PersistedTab::Relation(PersistedRelationTab {
                            id: relation_id,
                            object_id: CatalogId::new(second.id, CatalogKind::Table, ["users"]),
                            qualified_name: QualifiedName {
                                database: None,
                                schema: None,
                                object: "users".into(),
                            },
                            catalog_kind: CatalogKind::Table,
                            title: "users".into(),
                            view: lazydb::model::relation::RelationView::Ddl,
                        }),
                        PersistedTab::Dashboard {
                            dashboard_id,
                            page: Default::default(),
                            refresh_enabled: true,
                            redis_database: None,
                        },
                        PersistedTab::RedisBrowser {
                            tab_id: redis_id,
                            profile_id: second.id,
                            database: 0,
                            pattern: Vec::new(),
                        },
                    ],
                },
            ],
            active_console: relation_id,
            consoles: Vec::new(),
            tabs: Vec::new(),
            sql: vec![(console_id, "select 42".into())],
            recent_targets: Vec::new(),
        };
        let profiles = if missing_profile {
            vec![first.clone()]
        } else {
            vec![first.clone(), second]
        };
        let mut app = App::new(profiles.clone());
        app.restore_workspace(snapshot, Some(first.id));
        assert_eq!(
            app.connection.status,
            lazydb::model::workspace::ConnectionStatus::Disconnected
        );
        let temp = tempfile::TempDir::new().unwrap();
        let store =
            WorkspaceStore::new(temp.path().join("workspace.toml"), temp.path().join("sql"));
        // Close the cached active tab first to exercise active-tab repair,
        // then close every remaining view, including the last tab.
        let mut ids = vec![relation_id, console_id, dashboard_id];
        if !missing_profile {
            ids.push(redis_id);
        }
        for id in ids {
            let commands = app.update(Action::CloseTab(id));
            let snapshot = commands
                .into_iter()
                .find_map(|command| match command {
                    Command::PersistWorkspace { snapshot, .. } => Some(snapshot),
                    _ => None,
                })
                .expect("closing an offline tab must offer a save");
            store.save(&snapshot).unwrap();
            let mut restarted = App::new(profiles.clone());
            restarted.restore_workspace(store.load().unwrap().unwrap(), Some(first.id));
            assert!(restarted.tabs.iter().all(|tab| tab.id() != id));
            assert_eq!(restarted.tabs.len(), app.tabs.len());
            app = restarted;
        }
        assert!(app.tabs.is_empty());
        assert_eq!(app.editor_text(console_id).unwrap(), "select 42");
        assert!(
            !app.sql_editors
                .iter()
                .find(|record| record.id == console_id)
                .unwrap()
                .open
        );
    }
}

#[test]
fn sql_editor_target_selector_excludes_redis_profiles() {
    let relational = import_connection_url(":memory:", Some("redis"))
        .unwrap()
        .profile;
    let redis = import_connection_url("redis://localhost:6379", Some("cache"))
        .unwrap()
        .profile;
    let mut app = App::new(vec![relational.clone(), redis.clone()]);
    app.connection.profile_id = Some(relational.id);
    app.update(Action::NewConsole);
    let console_id = app.active_console().id;

    app.update(Action::OpenConsoleTargetSelector { console_id });
    let candidates = match app.overlay.as_ref().unwrap() {
        lazydb::model::workspace::Overlay::TargetSelector { candidates, .. } => candidates,
        overlay => panic!("unexpected overlay: {overlay:?}"),
    };
    assert!(candidates.iter().any(|target| {
        target
            .target()
            .is_some_and(|target| target.profile_id == relational.id)
    }));
    assert!(!candidates.iter().any(|target| {
        target
            .target()
            .is_some_and(|target| target.profile_id == redis.id)
    }));
}

#[test]
fn restored_relation_tab_is_not_loaded_before_connection_installation() {
    let profile = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let relation_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(relation_id),
            consoles: Vec::new(),
            tabs: vec![PersistedTab::Relation(
                lazydb::persistence::workspace::PersistedRelationTab {
                    id: relation_id,
                    object_id: lazydb::db::catalog::CatalogId::new(
                        profile.id,
                        lazydb::db::catalog::CatalogKind::Table,
                        ["users"],
                    ),
                    qualified_name: lazydb::db::catalog::QualifiedName {
                        database: None,
                        schema: Some("main".into()),
                        object: "users".into(),
                    },
                    catalog_kind: lazydb::db::catalog::CatalogKind::Table,
                    title: "users".into(),
                    view: lazydb::model::relation::RelationView::Data,
                },
            )],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);

    let restored = app.workspace_snapshot();
    assert!(matches!(
        &restored.profiles[0].tabs[0],
        PersistedTab::Relation(relation) if relation.id == relation_id
    ));
}

#[test]
fn restored_postgres_relation_prepares_its_schema_session() {
    let profile = import_connection_url(
        "postgresql://postgres:password@localhost:5432/lazydb_test",
        Some("postgres"),
    )
    .unwrap()
    .profile;
    let profile_id = profile.id;
    let relation_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile_id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id,
            active_tab: Some(relation_id),
            consoles: Vec::new(),
            tabs: vec![PersistedTab::Relation(
                lazydb::persistence::workspace::PersistedRelationTab {
                    id: relation_id,
                    object_id: CatalogId::new(
                        profile_id,
                        CatalogKind::Table,
                        ["lazydb_test", "test_schema", "all_types_test"],
                    ),
                    qualified_name: QualifiedName {
                        database: Some("lazydb_test".into()),
                        schema: Some("test_schema".into()),
                        object: "all_types_test".into(),
                    },
                    catalog_kind: CatalogKind::Table,
                    title: "all_types_test".into(),
                    view: lazydb::model::relation::RelationView::Data,
                },
            )],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);

    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };
    let commands = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: lazydb::profile::DatabaseKind::Postgres,
            version: "16".into(),
            database: "lazydb_test".into(),
            current_user: Some("postgres".into()),
        },
        mutation_capabilities: Default::default(),
    });
    let relation_connection_generation = commands.iter().find_map(|command| match command {
        Command::Connect {
            generation, target, ..
        } if target.database == "lazydb_test"
            && target.schema.as_deref() == Some("test_schema") =>
        {
            Some(*generation)
        }
        _ => None,
    });
    assert!(relation_connection_generation.is_some());
    assert!(commands.iter().any(|command| matches!(
        command,
        Command::Connect { target, .. }
            if target.database == "lazydb_test" && target.schema.as_deref() == Some("test_schema")
    )));
    assert!(matches!(
        &app.tabs[0],
        WorkspaceTab::Relation(tab)
            if matches!(
                tab.preparation,
                lazydb::model::relation::RelationPreparation::WaitingForSession { ref target }
                    if target.database == "lazydb_test"
                        && target.schema.as_deref() == Some("test_schema")
            )
    ));

    let follow_up = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation: relation_connection_generation.unwrap(),
        server: lazydb::db::ServerInfo {
            kind: lazydb::profile::DatabaseKind::Postgres,
            version: "16".into(),
            database: "lazydb_test".into(),
            current_user: Some("postgres".into()),
        },
        mutation_capabilities: Default::default(),
    });
    assert!(
        !follow_up
            .iter()
            .any(|command| matches!(command, Command::Connect { .. }))
    );
    assert!(matches!(
        &app.tabs[0],
        WorkspaceTab::Relation(tab)
            if matches!(tab.preparation, lazydb::model::relation::RelationPreparation::Idle)
    ));
}

#[test]
fn restored_relation_waits_for_catalog_before_loading() {
    let profile = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let database_id = CatalogId::new(profile_id, CatalogKind::Database, [":memory:"]);
    let schema_id = CatalogId::new(profile_id, CatalogKind::Schema, [":memory:", "main"]);
    let first_relation_id = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        [":memory:", "main", "first"],
    );
    let relation = |id: Uuid, title: &str| {
        PersistedTab::Relation(lazydb::persistence::workspace::PersistedRelationTab {
            id,
            object_id: CatalogId::new(profile_id, CatalogKind::Table, [":memory:", "main", title]),
            qualified_name: QualifiedName {
                database: Some(":memory:".into()),
                schema: Some("main".into()),
                object: title.into(),
            },
            catalog_kind: CatalogKind::Table,
            title: title.into(),
            view: lazydb::model::relation::RelationView::Data,
        })
    };
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile_id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id,
            active_tab: Some(first_id),
            consoles: Vec::new(),
            tabs: vec![relation(first_id, "first"), relation(second_id, "second")],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };

    let commands = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: lazydb::profile::DatabaseKind::Sqlite,
            version: "3".into(),
            database: ":memory:".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::LoadCatalogPage(_)))
            .count(),
        1
    );
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );
    assert!(matches!(app.tabs[0], WorkspaceTab::Relation(ref tab)
        if matches!(tab.data, lazydb::model::relation::RelationLoad::Loading { .. })));
    assert!(matches!(app.tabs[1], WorkspaceTab::Relation(ref tab)
        if !matches!(tab.data, lazydb::model::relation::RelationLoad::Ready(_))));

    let database_request = commands
        .iter()
        .find_map(|command| match command {
            Command::LoadCatalogPage(request) => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    let database = CatalogEntry::database(
        database_id.clone(),
        QualifiedName {
            database: Some(":memory:".into()),
            schema: None,
            object: ":memory:".into(),
        },
        "database",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &database_request,
            vec![database],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );

    let schema_target = CatalogTarget::schemas(database_id.clone()).unwrap();
    let schema_request = pending_catalog_request(&app, profile_id, &schema_target);
    let schema = CatalogEntry::schema(
        schema_id.clone(),
        database_id,
        QualifiedName {
            database: Some(":memory:".into()),
            schema: Some("main".into()),
            object: "main".into(),
        },
        "schema",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    // Lazy cascade: schemas auto-load but groups only preload when the schema
    // is expanded. Mark the schema expanded before loading Schemas so the
    // cascade fires Groups. Groups→Objects cascade requires the group to be
    // expanded, but `retain_existing_expansion` prunes expanded groups that
    // don't yet exist — so the test fixture manually fires the Objects request
    // via ExplorerLoadTarget instead of relying on the cascade.
    app.explorer
        .normalized
        .expanded
        .insert(lazydb::model::explorer::ExplorerNodeId::Catalog(
            schema_id.clone(),
        ));
    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(&schema_request, vec![schema], CatalogCount::Exact(1), None).unwrap(),
    ));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );

    let groups_target = CatalogTarget::groups(schema_id.clone()).unwrap();
    let groups_request = pending_catalog_request(&app, profile_id, &groups_target);
    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::groups(
            &groups_request,
            vec![CatalogGroupSummary {
                group: ObjectGroup::Tables,
                object_count: CatalogCount::Exact(1),
            }],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );

    let tables_target = CatalogTarget::objects(schema_id.clone(), ObjectGroup::Tables).unwrap();
    // Lazy cascade: the Objects cascade doesn't fire here because the group
    // isn't yet in `expanded` when the Groups page loads (expanded entries
    // get pruned if the group doesn't yet exist). Simulate the user expanding
    // the group by firing the Objects request explicitly.
    let tables_request = {
        let commands = app.update(Action::ExplorerLoadTarget(tables_target));
        match commands.as_slice() {
            [Command::LoadCatalogPage(request)] => request.clone(),
            commands => panic!("unexpected commands: {commands:?}"),
        }
    };
    let first_relation = CatalogEntry::relation(
        first_relation_id.clone(),
        schema_id,
        QualifiedName {
            database: Some(":memory:".into()),
            schema: Some("main".into()),
            object: "first".into(),
        },
        "table",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &tables_request,
            vec![first_relation],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );
    let commands = app.update(Action::ActivateTab(0));
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::LoadRelationPreview(request) if request.tab_id == first_id
    )));
}

fn pending_catalog_request(app: &App, profile_id: Uuid, target: &CatalogTarget) -> CatalogRequest {
    let owner = lazydb::model::explorer::owner_for_target(profile_id, target);
    app.explorer.normalized.profiles[&profile_id].pending_requests[&owner].clone()
}

#[test]
fn restored_ddl_relation_loads_after_catalog_identity_arrives() {
    let (mut app, _profile_id, _tab_id, relation_id, schema_id, tables_request) =
        restored_relation_at_tables_request(lazydb::model::relation::RelationView::Ddl);
    let relation = catalog_relation(relation_id.clone(), schema_id, "target");

    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &tables_request,
            vec![relation],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));

    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationDdl(_)))
    );
}

#[test]
fn restored_relation_waits_until_later_catalog_page_contains_identity() {
    let (mut app, profile_id, _tab_id, relation_id, schema_id, mut tables_request) =
        restored_relation_at_tables_request(lazydb::model::relation::RelationView::Data);
    let other_id = CatalogId::new(
        relation_id.profile_id(),
        CatalogKind::Table,
        [":memory:", "main", "other"],
    );
    let cursor = CatalogCursor::from_keyset("other", "other").unwrap();
    tables_request.page_size = 1;
    let owner = lazydb::model::explorer::owner_for_target(profile_id, &tables_request.key.target);
    app.explorer
        .normalized
        .profiles
        .get_mut(&profile_id)
        .unwrap()
        .pending_requests
        .insert(owner, tables_request.clone());
    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &tables_request,
            vec![catalog_relation(other_id, schema_id.clone(), "other")],
            CatalogCount::Exact(2),
            Some(cursor),
        )
        .unwrap(),
    ));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );
    let continuation = commands
        .iter()
        .find_map(|command| match command {
            Command::LoadCatalogPage(request) => Some(request.clone()),
            _ => None,
        })
        .expect("catalog continuation request");

    let commands = app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &continuation,
            vec![catalog_relation(relation_id.clone(), schema_id, "target")],
            CatalogCount::Exact(2),
            None,
        )
        .unwrap(),
    ));

    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadRelationPreview(_)))
    );
}

fn restored_relation_at_tables_request(
    view: lazydb::model::relation::RelationView,
) -> (App, Uuid, Uuid, CatalogId, CatalogId, CatalogRequest) {
    let profile = import_connection_url(":memory:", Some("restored"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let tab_id = Uuid::new_v4();
    let database_id = CatalogId::new(profile_id, CatalogKind::Database, [":memory:"]);
    let schema_id = CatalogId::new(profile_id, CatalogKind::Schema, [":memory:", "main"]);
    let relation_id = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        [":memory:", "main", "target"],
    );
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(profile_id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id,
            active_tab: Some(tab_id),
            consoles: Vec::new(),
            tabs: vec![PersistedTab::Relation(
                lazydb::persistence::workspace::PersistedRelationTab {
                    id: tab_id,
                    object_id: relation_id.clone(),
                    qualified_name: QualifiedName {
                        database: Some(":memory:".into()),
                        schema: Some("main".into()),
                        object: "target".into(),
                    },
                    catalog_kind: CatalogKind::Table,
                    title: "target".into(),
                    view,
                },
            )],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: Vec::new(),
    };
    let mut app = App::new(vec![profile]);
    app.restore_workspace(snapshot, None);
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };
    let commands = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: lazydb::profile::DatabaseKind::Sqlite,
            version: "3".into(),
            database: ":memory:".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });
    assert!(commands.iter().any(|command| matches!(
        command,
        Command::LoadRelationPreview(_) | Command::LoadRelationDdl(_)
    )));
    let database_request = commands
        .iter()
        .find_map(|command| match command {
            Command::LoadCatalogPage(request) => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    let database = CatalogEntry::database(
        database_id.clone(),
        QualifiedName {
            database: Some(":memory:".into()),
            schema: None,
            object: ":memory:".into(),
        },
        "database",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    app.update(Action::CatalogPageLoaded(
        CatalogPage::new(
            &database_request,
            vec![database],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));
    let schema_target = CatalogTarget::schemas(database_id.clone()).unwrap();
    let schema_request = pending_catalog_request(&app, profile_id, &schema_target);
    let schema = CatalogEntry::schema(
        schema_id.clone(),
        database_id,
        QualifiedName {
            database: Some(":memory:".into()),
            schema: Some("main".into()),
            object: "main".into(),
        },
        "schema",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    // Lazy cascade: schemas auto-load but groups only preload when the schema
    // is expanded. Mark the schema expanded before loading Schemas so the
    // cascade fires Groups. Groups→Objects cascade requires the group to be
    // expanded, but `retain_existing_expansion` prunes expanded groups that
    // don't yet exist — so the test fixture manually fires the Objects request
    // via ExplorerLoadTarget instead of relying on the cascade.
    app.explorer
        .normalized
        .expanded
        .insert(lazydb::model::explorer::ExplorerNodeId::Catalog(
            schema_id.clone(),
        ));
    app.update(Action::CatalogPageLoaded(
        CatalogPage::new(&schema_request, vec![schema], CatalogCount::Exact(1), None).unwrap(),
    ));
    let groups_target = CatalogTarget::groups(schema_id.clone()).unwrap();
    let groups_request = pending_catalog_request(&app, profile_id, &groups_target);
    app.update(Action::CatalogPageLoaded(
        CatalogPage::groups(
            &groups_request,
            vec![CatalogGroupSummary {
                group: ObjectGroup::Tables,
                object_count: CatalogCount::Exact(1),
            }],
            CatalogCount::Exact(1),
            None,
        )
        .unwrap(),
    ));
    let tables_target = CatalogTarget::objects(schema_id.clone(), ObjectGroup::Tables).unwrap();
    // Lazy cascade: simulate the user explicitly opening the tables group.
    let tables_request = {
        let commands = app.update(Action::ExplorerLoadTarget(tables_target));
        match commands.as_slice() {
            [Command::LoadCatalogPage(request)] => request.clone(),
            commands => panic!("unexpected commands: {commands:?}"),
        }
    };
    (
        app,
        profile_id,
        tab_id,
        relation_id,
        schema_id,
        tables_request,
    )
}

fn catalog_relation(id: CatalogId, schema: CatalogId, name: &str) -> CatalogEntry {
    CatalogEntry::relation(
        id,
        schema,
        QualifiedName {
            database: Some(":memory:".into()),
            schema: Some("main".into()),
            object: name.into(),
        },
        "table",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap()
}

#[test]
fn console_lifecycle_can_run_offline_with_a_profile_workspace() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    app.active_workspace_profile = Some(profile_id);

    let commands = app.update(Action::NewConsole);
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert_eq!(app.active_workspace_profile, Some(profile_id));
    assert_eq!(app.sql_editors.len(), 1);
    let id = app.active_console().id;
    app.update(Action::CloseActiveTab);

    assert!(app.tabs.is_empty());
    assert!(
        app.sql_editors
            .iter()
            .any(|record| record.id == id && !record.open)
    );

    let commands = app.update(Action::NewConsole);
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    let new_id = app.active_console().id;
    app.update(Action::RequestDeleteActiveConsole);
    app.update(Action::ToggleDeleteConsoleFocus);
    let commands = app.update(Action::ConfirmDeleteConsole);
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::Connect { .. }))
    );
    assert!(!app.sql_editors.iter().any(|record| record.id == new_id));
    assert!(app.tabs.is_empty());
}

#[test]
fn restored_console_target_cannot_cross_profile_boundaries() {
    let first = import_connection_url(":memory:", Some("first"))
        .unwrap()
        .profile;
    let second = import_connection_url(":memory:", Some("second"))
        .unwrap()
        .profile;
    let console_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        recent_targets: Vec::new(),
        active_profile: Some(first.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: first.id,
            active_tab: Some(console_id),
            consoles: vec![PersistedConsole {
                id: console_id,
                name: "cross-profile".into(),
                sql_file: format!("{console_id}.sql").into(),
                target: Some(
                    lazydb::model::execution_target::ExecutionTarget::from_profile(&second),
                ),
                transaction_mode: TransactionMode::Auto,
                open: true,
            }],
            tabs: vec![PersistedTab::Console { console_id }],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![(console_id, "select 1".into())],
    };
    let mut app = App::new(vec![first.clone(), second.clone()]);
    app.connection.profile_id = Some(first.id);
    app.restore_workspace(snapshot, Some(first.id));

    assert_eq!(app.active_workspace_profile, Some(first.id));
    assert_eq!(
        app.active_console().execution_target,
        Some(lazydb::model::execution_target::ExecutionTarget::from_profile(&second))
    );
}

#[test]
fn console_numbering_is_collision_free_within_each_workspace() {
    let profile = import_connection_url(":memory:", Some("saved"))
        .unwrap()
        .profile;
    let first_id = Uuid::new_v4();
    let snapshot = WorkspaceSnapshot {
        active_profile: Some(profile.id),
        profiles: vec![PersistedProfileWorkspace {
            profile_id: profile.id,
            active_tab: Some(first_id),
            consoles: vec![PersistedConsole {
                id: first_id,
                name: "console_1".into(),
                sql_file: format!("{first_id}.sql").into(),
                target: None,
                transaction_mode: TransactionMode::Auto,
                open: true,
            }],
            tabs: vec![PersistedTab::Console {
                console_id: first_id,
            }],
        }],
        active_console: Uuid::nil(),
        tabs: Vec::new(),
        consoles: Vec::new(),
        sql: vec![(first_id, String::new())],
        recent_targets: Vec::new(),
    };
    let mut app = App::new(vec![profile.clone()]);
    app.connection.profile_id = Some(profile.id);
    app.restore_workspace(snapshot, Some(profile.id));
    app.update(Action::NewConsole);

    assert_eq!(app.active_console().name, "console_2");
    assert_eq!(app.sql_editors.len(), 2);
}

fn persisted_console(
    id: Uuid,
    name: &str,
    open: bool,
    target: Option<lazydb::model::execution_target::ExecutionTarget>,
) -> PersistedConsole {
    PersistedConsole {
        id,
        name: name.into(),
        sql_file: format!("{id}.sql").into(),
        target,
        transaction_mode: TransactionMode::Auto,
        open,
    }
}

fn profile_workspace(
    profile_id: Uuid,
    console_id: Uuid,
    active_tab: Option<Uuid>,
    target: Option<lazydb::model::execution_target::ExecutionTarget>,
) -> PersistedProfileWorkspace {
    PersistedProfileWorkspace {
        profile_id,
        active_tab,
        consoles: vec![PersistedConsole {
            id: console_id,
            name: "console".into(),
            sql_file: format!("{console_id}.sql").into(),
            target,
            transaction_mode: TransactionMode::Auto,
            open: true,
        }],
        tabs: vec![PersistedTab::Console { console_id }],
    }
}
