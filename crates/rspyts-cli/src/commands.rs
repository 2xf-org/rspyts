//! Command execution and user-visible reporting.

use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use crate::cli::{Command, ProjectArgs};
use crate::init;
use crate::output::SourceWatcher;
use crate::project::{Project, build, check};

/// Execute one parsed command using explicit output streams.
pub(crate) fn execute(
    command: Command,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<()> {
    match command {
        Command::Init(args) => write_json(stdout, &init::create(&args.path)?),
        Command::Build(args) => {
            let project = read_project(&args)?;
            write_json(stdout, &build(&project)?)
        }
        Command::Check(args) => {
            let project = read_project(&args)?;
            check(&project)?;
            write_json(
                stdout,
                &json!({
                    "status": "ok",
                    "output": project.output(),
                }),
            )
        }
        Command::Watch(args) => watch(read_project(&args)?, stdout, stderr),
    }
}

fn read_project(args: &ProjectArgs) -> Result<Project> {
    Project::read(&args.manifest_path, args.output.as_deref())
}

/// Build once, then rebuild after each observed source change.
///
/// The loop deliberately remains at the process boundary: a fatal watcher
/// error ends the command, while a build error is reported and waits for the
/// next source change.
fn watch(project: Project, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<()> {
    build(&project)?;
    writeln!(
        stdout,
        "rspyts is watching {}",
        project.workspace_root.display()
    )?;
    let mut watcher = SourceWatcher::new(&project.workspace_root)?;
    loop {
        thread::sleep(Duration::from_millis(500));
        if watcher.changed()? {
            match build(&project) {
                Ok(_) => writeln!(stdout, "rspyts rebuilt {}", project.output().display())?,
                Err(error) => writeln!(stderr, "rspyts build failed: {error:#}")?,
            }
        }
    }
}

fn write_json(output: &mut dyn Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *output, value)?;
    writeln!(output)?;
    Ok(())
}
