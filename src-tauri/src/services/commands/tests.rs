//! Remote command tests against the scripted in-memory shell.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::domain::command::{CommandEvent, CommandStream};
use crate::domain::session::{ConnectTarget, SessionInfo};
use crate::error::ErrorCode;
use crate::remote::memory::Script;
use crate::services::test_support::{Env, PASSWORD, null_sink, sftp_input};

type Events = Arc<Mutex<Vec<CommandEvent>>>;

fn recorder() -> (Events, CommandSink) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (events, Arc::new(move |e| sink.lock().unwrap().push(e)))
}

async fn connected(env: &Env) -> (Arc<Session>, SessionInfo) {
    let info = env
        .manager
        .connect(
            env.db(),
            ConnectTarget::Adhoc { site: sftp_input() },
            Some(PASSWORD.into()),
            false,
            null_sink(),
        )
        .await
        .unwrap();
    (env.manager.get(&info.id).unwrap(), info)
}

fn is_terminal(event: &CommandEvent) -> bool {
    matches!(
        event,
        CommandEvent::Exit { .. } | CommandEvent::Failed { .. }
    )
}

/// Waits for the terminal event, then checks it is the only one and the last.
async fn finished(events: &Events) -> Vec<CommandEvent> {
    for _ in 0..1000 {
        if events.lock().unwrap().iter().any(is_terminal) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    let seen = events.lock().unwrap().clone();
    assert_eq!(
        seen.iter().filter(|e| is_terminal(e)).count(),
        1,
        "{seen:?}"
    );
    assert!(seen.last().is_some_and(is_terminal), "{seen:?}");
    seen
}

fn text(events: &[CommandEvent], wanted: CommandStream) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            CommandEvent::Output { stream, text } if *stream == wanted => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn exit_of(events: &[CommandEvent]) -> crate::domain::command::CommandExit {
    match events.last() {
        Some(CommandEvent::Exit { exit, .. }) => exit.clone(),
        other => panic!("expected an exit, got {other:?}"),
    }
}

fn shell_env() -> Env {
    let env = Env::new();
    env.server.enable_shell();
    env
}

#[test]
fn quoting_is_posix_single_quotes() {
    assert_eq!(shell_quote("/var/www"), "'/var/www'");
    assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    assert_eq!(shell_quote("$HOME `id` \"x\""), "'$HOME `id` \"x\"'");
    assert_eq!(shell_quote("line\nbreak"), "'line\nbreak'");
    assert_eq!(shell_quote("/srv/olá/日本"), "'/srv/olá/日本'");
    assert_eq!(shell_quote("''"), r"''\'''\'''");
    assert_eq!(
        expand("du -sh {dir}; ls {dir}", Some("/a b")).unwrap(),
        "du -sh '/a b'; ls '/a b'"
    );
    assert_eq!(expand("uptime", None).unwrap(), "uptime");
    assert_eq!(expand("uptime", Some("/x")).unwrap(), "uptime");
    for (command, dir) in [
        ("ls {dir}", None),
        ("ls {dir}", Some("")),
        ("  ", None),
        ("a\0b", None),
    ] {
        assert_eq!(
            expand(command, dir).unwrap_err().code(),
            ErrorCode::Validation,
            "{command:?}"
        );
    }
}

#[tokio::test]
async fn runs_stream_output_then_one_exit() {
    let env = shell_env();
    env.server.script("echo hi", Script::stdout("hi\n", 0));
    let (session, info) = connected(&env).await;
    assert!(info.capabilities.exec);
    let (events, sink) = recorder();
    let started = run(&session, "echo hi", None, sink).unwrap();
    assert_eq!(started.command, "echo hi");
    let seen = finished(&events).await;
    assert_eq!(text(&seen, CommandStream::Stdout), "hi\n");
    let exit = exit_of(&seen);
    assert_eq!((exit.code, exit.cancelled), (Some(0), false));
    assert!(!session.runs.contains(&started.run_id));
    // Stopping a finished (or unknown) run is fine.
    env.manager.cancel_run(&started.run_id);
    env.manager.cancel_run("nope");
}

#[tokio::test]
async fn dir_is_expanded_quoted() {
    let env = shell_env();
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    let started = run(
        &session,
        "ls -la {dir} && du -s {dir}",
        Some("/srv/a b/it's"),
        sink,
    )
    .unwrap();
    let expected = r"ls -la '/srv/a b/it'\''s' && du -s '/srv/a b/it'\''s'";
    assert_eq!(started.command, expected);
    finished(&events).await;
    assert_eq!(env.server.executed(), [expected]);
    // Unknown commands: the shell's own error on stderr and exit 127.
    let seen = events.lock().unwrap().clone();
    assert!(text(&seen, CommandStream::Stderr).contains("not found"));
    assert_eq!(exit_of(&seen).code, Some(127));
}

#[tokio::test]
async fn output_is_decoded_cleaned_and_split_by_stream() {
    let env = shell_env();
    env.server.script(
        "check",
        Script {
            output: vec![
                (CommandStream::Stdout, b"\x1b[32mok\x1b[0m caf\xc3".to_vec()),
                (CommandStream::Stdout, b"\xa9 \xe6\x97".to_vec()),
                (CommandStream::Stdout, b"\xa5\r\n".to_vec()),
                (CommandStream::Stderr, b"\x1b[1;31mwarn\x1b[".to_vec()),
                (CommandStream::Stderr, b"0m: low disk\r\n".to_vec()),
                (CommandStream::Stdout, b"10%\r50%\r100%\n".to_vec()),
            ],
            code: Some(3),
            ..Script::default()
        },
    );
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    run(&session, "check", None, sink).unwrap();
    let seen = finished(&events).await;
    assert_eq!(text(&seen, CommandStream::Stdout), "ok café 日\n100%\n");
    assert_eq!(text(&seen, CommandStream::Stderr), "warn: low disk\n");
    assert_eq!(exit_of(&seen).code, Some(3));
}

#[tokio::test]
async fn output_is_batched() {
    let env = shell_env();
    let chunks = 300;
    env.server.script(
        "chatty",
        Script {
            output: vec![(CommandStream::Stdout, b"line\n".to_vec()); chunks],
            delay: Some(Duration::from_millis(1)),
            code: Some(0),
            ..Script::default()
        },
    );
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    let started = Instant::now();
    run(&session, "chatty", None, sink).unwrap();
    let seen = finished(&events).await;
    let elapsed = started.elapsed();
    let outputs = seen
        .iter()
        .filter(|e| matches!(e, CommandEvent::Output { .. }))
        .count();
    assert_eq!(text(&seen, CommandStream::Stdout), "line\n".repeat(chunks));
    // At most one event per batch window (plus the first tick and the final flush). Bounded by
    // time, not chunk count: a 1 ms sleep lasts ~15 ms on Windows, so the run is ~15x longer.
    let windows = (elapsed.as_millis() / BATCH.as_millis()) as usize;
    assert!(
        outputs <= windows + 2 && outputs < chunks,
        "{outputs} output events for {chunks} chunks in {elapsed:?}"
    );
}

#[tokio::test]
async fn partial_lines_show_up_before_the_command_ends() {
    let env = shell_env();
    env.server.script(
        "prompt",
        Script {
            output: vec![
                (CommandStream::Stdout, b"waiting".to_vec()),
                (CommandStream::Stdout, b"".to_vec()),
                (CommandStream::Stdout, b" done\n".to_vec()),
            ],
            delay: Some(Duration::from_millis(150)),
            code: Some(0),
            ..Script::default()
        },
    );
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    run(&session, "prompt", None, sink).unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        text(&events.lock().unwrap(), CommandStream::Stdout),
        "waiting"
    );
    let seen = finished(&events).await;
    assert_eq!(text(&seen, CommandStream::Stdout), "waiting done\n");
}

fn endless(ignores_term: bool) -> Script {
    Script {
        output: vec![(CommandStream::Stdout, b"tick\n".to_vec())],
        delay: Some(Duration::from_millis(10)),
        endless: true,
        ignores_term,
        ..Script::default()
    }
}

#[tokio::test]
async fn stop_cancels_once() {
    let env = shell_env();
    env.server.script("tail -f log", endless(false));
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    let started = run(&session, "tail -f log", None, sink).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    env.manager.cancel_run(&started.run_id);
    env.manager.cancel_run(&started.run_id);
    let seen = finished(&events).await;
    let exit = exit_of(&seen);
    assert!(exit.cancelled);
    assert_eq!(exit.signal.as_deref(), Some("TERM"));
    assert!(text(&seen, CommandStream::Stdout).starts_with("tick\n"));
    assert!(!text(&seen, CommandStream::Stderr).contains("still be running"));
}

#[tokio::test]
async fn a_forced_stop_says_the_program_may_still_run() {
    let env = shell_env();
    env.server.script("stubborn", endless(true));
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    let started = run(&session, "stubborn", None, sink).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    env.manager.cancel_run(&started.run_id);
    let seen = finished(&events).await;
    let exit = exit_of(&seen);
    assert!(exit.cancelled && exit.signal.is_none() && exit.code.is_none());
    assert!(text(&seen, CommandStream::Stderr).ends_with(STILL_RUNNING_NOTE));
}

#[tokio::test]
async fn closing_the_session_stops_its_runs() {
    let env = shell_env();
    env.server.script("tail -f log", endless(false));
    let (session, info) = connected(&env).await;
    let (events, sink) = recorder();
    run(&session, "tail -f log", None, sink).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    env.manager.disconnect(&info.id).await.unwrap();
    let seen = finished(&events).await;
    assert!(exit_of(&seen).cancelled);
}

#[tokio::test]
async fn too_much_output_stops_the_run() {
    let env = shell_env();
    env.server.script(
        "yes",
        Script {
            output: vec![(CommandStream::Stdout, vec![b'y'; 100])],
            endless: true,
            ..Script::default()
        },
    );
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    run_capped(&session, "yes", None, sink, 1000).unwrap();
    let seen = finished(&events).await;
    assert!(text(&seen, CommandStream::Stdout).len() <= 1000);
    assert!(text(&seen, CommandStream::Stderr).contains("Gem stopped this command"));
    assert!(exit_of(&seen).cancelled);
}

#[tokio::test]
async fn ftp_has_no_shell_and_dropped_sessions_refuse() {
    let env = Env::new(); // no shell: like FTP
    let (session, info) = connected(&env).await;
    assert!(!info.capabilities.exec);
    let (_, sink) = recorder();
    let error = run(&session, "uptime", None, Arc::clone(&sink)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Unsupported);
    assert_eq!(error.to_string(), "Commands need an SSH (SFTP) connection");

    let env = shell_env();
    let (session, _) = connected(&env).await;
    env.server.drop_connection();
    let _ = session.observe(session.fs().unwrap().list("/").await);
    assert_eq!(
        run(&session, "uptime", None, sink).unwrap_err().code(),
        ErrorCode::Disconnected
    );
}

#[tokio::test]
async fn failures_are_the_single_terminal_event() {
    let env = shell_env();
    env.server.script(
        "flaky",
        Script {
            output: vec![(CommandStream::Stdout, b"partial\n".to_vec())],
            fail: Some("connection reset by peer".into()),
            ..Script::default()
        },
    );
    let (session, _) = connected(&env).await;
    let (events, sink) = recorder();
    run(&session, "flaky", None, sink).unwrap();
    let seen = finished(&events).await;
    assert_eq!(text(&seen, CommandStream::Stdout), "partial\n");
    match seen.last() {
        Some(CommandEvent::Failed { error }) => assert_eq!(error.code, ErrorCode::Connection),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn too_many_channels_is_a_clear_failure() {
    let env = shell_env();
    env.server.script("tail -f log", endless(false));
    env.server.limit_execs(1);
    let (session, _) = connected(&env).await;
    let (first, sink) = recorder();
    let running = run(&session, "tail -f log", None, sink).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (second, sink) = recorder();
    run(&session, "uptime", None, sink).unwrap();
    let seen = finished(&second).await;
    match seen.last() {
        Some(CommandEvent::Failed { error }) => {
            assert_eq!(error.code, ErrorCode::Remote);
            assert_eq!(error.message, NO_MORE_CHANNELS);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        session.state(),
        crate::domain::session::SessionState::Connected
    );
    env.manager.cancel_run(&running.run_id);
    finished(&first).await;
}
