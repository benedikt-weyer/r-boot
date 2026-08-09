use std::path::PathBuf;

pub enum Command {
    /// Print the current default entry, timeout, and menu entries.
    Show,
    /// Change the default boot entry.
    SetDefault(String),
    /// Change the menu timeout, in seconds.
    SetTimeout(u32),
}

pub struct Args {
    /// EFI system partition mount point (matches r-boot-conf-builder's `-d`).
    pub esp: PathBuf,
    pub command: Command,
}

impl Args {
    pub fn usage() {
        eprintln!(
            "usage: r-boot-cli [-d <esp-mount-point>] <command>\n\
             \n\
             commands:\n  \
             show                    print the current r-boot configuration\n  \
             set-default <id>        change the default boot entry\n  \
             set-timeout <seconds>   change the menu timeout"
        );
    }

    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut esp = PathBuf::from("/boot");
        let mut command = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-d" => {
                    esp = PathBuf::from(
                        args.next().ok_or_else(|| "-d: missing argument".to_string())?,
                    )
                }
                "show" => command = Some(Command::Show),
                "set-default" => {
                    let id = args
                        .next()
                        .ok_or_else(|| "set-default: missing <id> argument".to_string())?;
                    command = Some(Command::SetDefault(id));
                }
                "set-timeout" => {
                    let seconds = args
                        .next()
                        .ok_or_else(|| "set-timeout: missing <seconds> argument".to_string())?
                        .parse::<u32>()
                        .map_err(|e| format!("set-timeout: {e}"))?;
                    command = Some(Command::SetTimeout(seconds));
                }
                other => return Err(format!("unrecognized argument: {other}")),
            }
        }

        Ok(Args {
            esp,
            command: command.ok_or("a command is required")?,
        })
    }
}
