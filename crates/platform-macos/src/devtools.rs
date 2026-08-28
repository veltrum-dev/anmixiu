use std::{fmt::Write, process, time::Duration};

use anmixiu_core::{
    DEVTOOLS_SOCKET_PATH, ElementId, ElementNode, GlobalElementId, InteractiveElement,
    ParentElement, Styled,
};
use anmixiu_layout_taffy::LayoutNodeId;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixStream, unix::OwnedReadHalf},
    runtime::Handle,
    sync::mpsc,
    task::JoinHandle,
};

use crate::BuiltFrame;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const OUTBOUND_CAPACITY: usize = 32;
const COMMAND_CAPACITY: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DevToolsCommand {
    RequestTree,
    Preview(String),
    PreviewNode(u64),
    ClearPreview,
    Inspect(String),
    InspectNode(u64),
    ClearInspection,
}

/// Application-side discovery and command agent for Anmixiu Dev Tools.
pub(crate) struct DevToolsAgent {
    outbound: mpsc::Sender<String>,
    task: JoinHandle<()>,
}

impl DevToolsAgent {
    pub(crate) fn connect(
        handle: &Handle,
        app_name: &str,
        wake_appkit: impl Fn() + Send + Sync + 'static,
    ) -> (Self, mpsc::Receiver<DevToolsCommand>) {
        let hello = hello_message(app_name);
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (commands, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let task = handle.spawn(run_agent(hello, outbound_rx, commands, wake_appkit));
        (Self { outbound, task }, command_rx)
    }

    pub(crate) fn publish_tree(&self, element: &ElementNode, frame: &BuiltFrame) {
        let mut message = String::from("ANMIXIU_TREE_BEGIN\n");
        let mut path = Vec::new();
        let mut next_index = 0;
        append_nodes(
            element,
            frame,
            &mut path,
            None,
            0,
            &mut next_index,
            &mut message,
        );
        message.push_str("ANMIXIU_TREE_END\n");
        let _ = self.outbound.try_send(message);
    }
}

impl Drop for DevToolsAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run_agent<F>(
    hello: String,
    mut outbound: mpsc::Receiver<String>,
    commands: mpsc::Sender<DevToolsCommand>,
    wake_appkit: F,
) where
    F: Fn() + Send + Sync + 'static,
{
    loop {
        match UnixStream::connect(DEVTOOLS_SOCKET_PATH).await {
            Ok(stream) => {
                let (reader, mut writer) = stream.into_split();
                if writer.write_all(hello.as_bytes()).await.is_err() {
                    continue;
                }
                let mut reader = BufReader::new(reader);
                if run_connection(
                    &mut reader,
                    &mut writer,
                    &mut outbound,
                    &commands,
                    &wake_appkit,
                )
                .await
                {
                    return;
                }
            }
            Err(_) => {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            }
        }
    }
}

async fn run_connection<F>(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    outbound: &mut mpsc::Receiver<String>,
    commands: &mpsc::Sender<DevToolsCommand>,
    wake_appkit: &F,
) -> bool
where
    F: Fn() + Send + Sync + 'static,
{
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            message = outbound.recv() => {
                let Some(message) = message else {
                    return true;
                };
                if writer.write_all(message.as_bytes()).await.is_err() {
                    return false;
                }
            }
            result = read_line(reader) => {
                let Some(line) = result else {
                    return false;
                };
                if let Some(command) = parse_command(&line)
                    && commands.try_send(command).is_ok()
                {
                    wake_appkit();
                }
            }
            _ = heartbeat.tick() => {
                if writer.write_all(b"ANMIXIU_HEARTBEAT\n").await.is_err() {
                    return false;
                }
            }
        }
    }
}

async fn read_line(reader: &mut BufReader<OwnedReadHalf>) -> Option<String> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await.ok()?;
    (bytes != 0).then_some(line)
}

fn parse_command(line: &str) -> Option<DevToolsCommand> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == "ANMIXIU_REQUEST_TREE" {
        return Some(DevToolsCommand::RequestTree);
    }
    if line == "ANMIXIU_CLEAR_INSPECTION" {
        return Some(DevToolsCommand::ClearInspection);
    }
    if line == "ANMIXIU_CLEAR_PREVIEW" {
        return Some(DevToolsCommand::ClearPreview);
    }
    if let Some(value) = line.strip_prefix("ANMIXIU_PREVIEW\t") {
        return Some(DevToolsCommand::Preview(value.to_owned()));
    }
    if let Some(value) = line.strip_prefix("ANMIXIU_PREVIEW_NODE\t") {
        return value.parse().ok().map(DevToolsCommand::PreviewNode);
    }
    if let Some(value) = line.strip_prefix("ANMIXIU_INSPECT_NODE\t") {
        return value.parse().ok().map(DevToolsCommand::InspectNode);
    }
    line.strip_prefix("ANMIXIU_INSPECT\t")
        .map(|value| DevToolsCommand::Inspect(value.to_owned()))
}

fn append_nodes(
    element: &ElementNode,
    frame: &BuiltFrame,
    path: &mut Vec<ElementId>,
    parent: Option<u64>,
    depth: u32,
    next_index: &mut u64,
    output: &mut String,
) {
    let index = *next_index;
    *next_index = next_index.saturating_add(1);
    let own_id = element.element_id().cloned();
    if let Some(id) = own_id.as_ref() {
        path.push(id.clone());
    }
    let global_id = (!path.is_empty()).then(|| GlobalElementId::new(path.iter().cloned()));
    let bounds = frame.layout.bounds(LayoutNodeId(index));
    let (x, y, width, height) = bounds.map_or((0.0, 0.0, 0.0, 0.0), |bounds| {
        (
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        )
    });
    let text = element.text_content().unwrap_or_default();
    let _ = writeln!(
        output,
        "ANMIXIU_NODE\t{index}\t{depth}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        parent.map_or_else(String::new, |value| value.to_string()),
        global_id.map_or_else(String::new, |value| value.to_string()),
        element.kind_name(),
        sanitize_field(text),
        x,
        y,
        width,
        height,
        element.style_ref().padding.value(),
        element.style_ref().gap.value(),
    );
    // A text-bearing element is a layout leaf: `project_node` in the frame builder assigns no
    // LayoutNodeId to its children and does not recurse. DevTools must apply the identical rule or
    // the shared `index` counter drifts, and every node after the offending one reads another
    // node's bounds via `LayoutNodeId(index)`.
    if element.text_content().is_none() {
        for child in element.children_ref() {
            append_nodes(
                child,
                frame,
                path,
                Some(index),
                depth.saturating_add(1),
                next_index,
                output,
            );
        }
    }
    if own_id.is_some() {
        path.pop();
    }
}

fn sanitize_field(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            matches!(character, '\t' | '\r' | '\n')
                .then_some(' ')
                .unwrap_or(character)
        })
        .collect()
}

fn hello_message(app_name: &str) -> String {
    format!(
        "ANMIXIU_HELLO\t{}\t{}\t{}\n",
        process::id(),
        sanitize_field(app_name),
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::{DevToolsCommand, hello_message, parse_command};

    #[test]
    fn hello_message_contains_the_app_identity() {
        let message = hello_message("Counter\nMVP");
        assert!(message.starts_with("ANMIXIU_HELLO\t"));
        assert!(message.contains("\tCounter MVP\t"));
        assert!(message.ends_with('\n'));
    }

    #[test]
    fn parses_devtools_commands() {
        assert_eq!(
            parse_command("ANMIXIU_REQUEST_TREE\n"),
            Some(DevToolsCommand::RequestTree)
        );
        assert_eq!(
            parse_command("ANMIXIU_INSPECT\troot/button\n"),
            Some(DevToolsCommand::Inspect("root/button".to_owned()))
        );
        assert_eq!(
            parse_command("ANMIXIU_PREVIEW\troot/button\n"),
            Some(DevToolsCommand::Preview("root/button".to_owned()))
        );
        assert_eq!(
            parse_command("ANMIXIU_PREVIEW_NODE\t7\n"),
            Some(DevToolsCommand::PreviewNode(7))
        );
        // INSPECT_NODE must parse as by-index, not be mis-split by the INSPECT prefix.
        assert_eq!(
            parse_command("ANMIXIU_INSPECT_NODE\t12\n"),
            Some(DevToolsCommand::InspectNode(12))
        );
        assert_eq!(
            parse_command("ANMIXIU_CLEAR_PREVIEW\n"),
            Some(DevToolsCommand::ClearPreview)
        );
        assert_eq!(
            parse_command("ANMIXIU_CLEAR_INSPECTION\n"),
            Some(DevToolsCommand::ClearInspection)
        );
        assert_eq!(parse_command("UNKNOWN\n"), None);
    }
}
