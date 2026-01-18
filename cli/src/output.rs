use colored::Colorize;
use reinschrift_core::TodoItem;
use chrono::{Local, NaiveDateTime};

pub struct OutputContext {
    pub json: bool,
    pub color: bool,
}

impl OutputContext {
    pub fn new(json: bool, color: bool) -> Self {
        if !color {
            colored::control::set_override(false);
        }
        Self { json, color }
    }

    pub fn success(&self, msg: &str) {
        if self.json {
            println!(r#"{{"status":"success","message":"{}"}}"#, escape_json(msg));
        } else {
            println!("{} {}", "✓".green().bold(), msg);
        }
    }

    pub fn error(&self, msg: &str) {
        if self.json {
            eprintln!(r#"{{"status":"error","message":"{}"}}"#, escape_json(msg));
        } else {
            eprintln!("{} {}", "✗".red().bold(), msg);
        }
    }

    pub fn info(&self, msg: &str) {
        if self.json {
            println!(r#"{{"status":"info","message":"{}"}}"#, escape_json(msg));
        } else {
            println!("{}", msg.dimmed());
        }
    }

    pub fn print_todo(&self, item: &TodoItem) {
        if self.json {
            if let Ok(json) = serde_json::to_string(item) {
                println!("{}", json);
            }
        } else {
            print_todo_colored(item);
        }
    }

    pub fn print_todos(&self, items: &[TodoItem]) {
        if self.json {
            if let Ok(json) = serde_json::to_string(items) {
                println!("{}", json);
            }
        } else {
            for item in items {
                print_todo_colored(item);
            }
        }
    }

    pub fn print_todos_grouped(&self, items: &[TodoItem], group_fn: impl Fn(&TodoItem) -> String) {
        if self.json {
            if let Ok(json) = serde_json::to_string(items) {
                println!("{}", json);
            }
        } else {
            let mut current_group: Option<String> = None;
            for item in items {
                let group = group_fn(item);
                if current_group.as_ref() != Some(&group) {
                    if current_group.is_some() {
                        println!();
                    }
                    println!("{}", group.bold().underline());
                    current_group = Some(group);
                }
                print_todo_colored(item);
            }
        }
    }
}

fn print_todo_colored(item: &TodoItem) {
    let checkbox = if item.done {
        "[x]".green()
    } else {
        "[ ]".white()
    };

    let title = if item.done {
        item.title.dimmed().strikethrough()
    } else {
        item.title.normal()
    };

    let mut line = format!("{} {}", checkbox, title);

    if !item.projects.is_empty() {
        let projects_str = item.projects.iter()
            .map(|p| format!("+{}", p))
            .collect::<Vec<_>>()
            .join(" ");
        line.push_str(&format!(" {}", projects_str.cyan()));
    }

    if !item.contexts.is_empty() {
        let contexts_str = item.contexts.iter()
            .map(|c| format!("@{}", c))
            .collect::<Vec<_>>()
            .join(" ");
        line.push_str(&format!(" {}", contexts_str.yellow()));
    }

    if let Some(due) = &item.due {
        let due_str = format_due(due);
        line.push_str(&format!(" {}", due_str));
    }

    if let Some(recurrence) = &item.recurrence {
        line.push_str(&format!(" {}", format!("rec:{}", recurrence).magenta()));
    }

    if let Some(marker) = &item.key.marker {
        line.push_str(&format!(" {}", format!("^{}", marker).dimmed()));
    } else {
        line.push_str(&format!(" {}", format!("#{}", item.key.line_index).dimmed()));
    }

    println!("  {}", line);
}

fn format_due(due: &NaiveDateTime) -> colored::ColoredString {
    let today = Local::now().date_naive();
    let due_date = due.date();
    let date_str = due.format("%Y-%m-%d").to_string();

    if due_date < today {
        format!("due:{}", date_str).red().bold()
    } else if due_date == today {
        format!("due:{}", date_str).yellow().bold()
    } else {
        format!("due:{}", date_str).blue()
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub fn format_id(item: &TodoItem) -> String {
    if let Some(marker) = &item.key.marker {
        format!("^{}", marker)
    } else {
        format!("#{}", item.key.line_index)
    }
}
