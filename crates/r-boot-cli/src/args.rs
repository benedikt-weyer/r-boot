use std::path::PathBuf;

pub enum Command {
    /// Print the current default entry, timeout, and menu entries.
    Show,
    /// Change the default boot entry.
    SetDefault(String),
    /// Change the menu timeout, in seconds.
    SetTimeout(u32),
    /// Change spinner output to off, text, or graphical.
    SetSpinner(String),
    /// Show or hide the firmware logo while using the graphical spinner.
    SetLogo(bool),
    /// Remove a boot entry, optionally deleting its kernel/initramfs files.
    Remove { id: String, purge: bool },
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
             set-timeout <seconds>   change the menu timeout\n  \
             set-spinner <mode>      set spinner: off, text, or graphical\n  \
             set-logo <on|off>       show or hide the firmware logo\n  \
             remove <id> [--purge]   remove a boot entry (--purge also deletes\n  \
             \x20                        its kernel/initramfs files)"
        );
    }

    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut esp = PathBuf::from("/boot");
        let mut command = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-d" => {
                    esp = PathBuf::from(
                        args.next()
                            .ok_or_else(|| "-d: missing argument".to_string())?,
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
                "set-spinner" => {
                    let mode = args
                        .next()
                        .ok_or_else(|| "set-spinner: missing <mode> argument".to_string())?;
                    if !matches!(mode.as_str(), "off" | "text" | "graphical") {
                        return Err("set-spinner: mode must be off, text, or graphical".to_string());
                    }
                    command = Some(Command::SetSpinner(mode));
                }
                "set-logo" => {
                    let visible = match args
                        .next()
                        .ok_or_else(|| "set-logo: missing <on|off>".to_string())?
                        .as_str()
                    {
                        "on" => true,
                        "off" => false,
                        _ => return Err("set-logo: value must be on or off".to_string()),
                    };
                    command = Some(Command::SetLogo(visible));
                }
                "remove" => {
                    let id = args
                        .next()
                        .ok_or_else(|| "remove: missing <id> argument".to_string())?;
                    let purge = match args.next() {
                        None => false,
                        Some(flag) if flag == "--purge" => true,
                        Some(other) => return Err(format!("unrecognized argument: {other}")),
                    };
                    command = Some(Command::Remove { id, purge });
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
