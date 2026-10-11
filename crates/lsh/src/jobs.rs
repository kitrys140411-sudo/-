//! Job table for job control.

use crate::sys::{self, WaitStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcState {
    Running,
    Stopped(i32),
    Done(WaitStatus),
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: libc::pid_t,
    pub state: ProcState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: usize,
    pub pgid: libc::pid_t,
    pub procs: Vec<Proc>,
    pub text: String,
    /// State last reported to the user.
    pub reported: Option<JobState>,
    /// Terminal modes saved when the job was stopped.
    pub termios: Option<sys::Termios>,
    pub background: bool,
}

impl Job {
    pub fn state(&self) -> JobState {
        if self.procs.iter().all(|p| matches!(p.state, ProcState::Done(_))) {
            JobState::Done
        } else if self.procs.iter().any(|p| matches!(p.state, ProcState::Stopped(_)))
            && self.procs.iter().all(|p| !matches!(p.state, ProcState::Running))
        {
            JobState::Stopped
        } else {
            JobState::Running
        }
    }

    /// Exit status of the job (last process, or last failing one with pipefail).
    pub fn status(&self, pipefail: bool) -> i32 {
        let codes: Vec<i32> = self
            .procs
            .iter()
            .map(|p| match p.state {
                ProcState::Done(ws) => ws.code(),
                ProcState::Stopped(sig) => 128 + sig,
                ProcState::Running => 0,
            })
            .collect();
        if pipefail {
            codes.iter().rev().find(|&&c| c != 0).copied().unwrap_or(0)
        } else {
            codes.last().copied().unwrap_or(0)
        }
    }

    pub fn describe_state(&self) -> String {
        match self.state() {
            JobState::Running => "Running".into(),
            JobState::Stopped => {
                let sig = self.procs.iter().find_map(|p| match p.state {
                    ProcState::Stopped(s) => Some(s),
                    _ => None,
                });
                match sig {
                    Some(libc::SIGTTIN) => "Stopped (tty input)".into(),
                    Some(libc::SIGTTOU) => "Stopped (tty output)".into(),
                    _ => "Stopped".into(),
                }
            }
            JobState::Done => match self.procs.last().map(|p| p.state) {
                Some(ProcState::Done(WaitStatus::Exited(0))) => "Done".into(),
                Some(ProcState::Done(WaitStatus::Exited(c))) => format!("Exit {c}"),
                Some(ProcState::Done(WaitStatus::Signaled(s, core))) => {
                    let d = sys::signal_description(s).map(String::from).unwrap_or_else(|| format!("Signal {s}"));
                    if core { format!("{d} (core dumped)") } else { d }
                }
                _ => "Done".into(),
            },
        }
    }
}

#[derive(Default)]
pub struct JobTable {
    pub jobs: Vec<Job>,
    /// Most recently stopped/backgrounded job ids (for `%+` / `%-`).
    pub order: Vec<usize>,
}

impl JobTable {
    pub fn add(&mut self, pgid: libc::pid_t, pids: &[libc::pid_t], text: &str, background: bool) -> usize {
        let id = (1..).find(|i| !self.jobs.iter().any(|j| j.id == *i)).unwrap();
        self.jobs.push(Job {
            id,
            pgid,
            procs: pids.iter().map(|&pid| Proc { pid, state: ProcState::Running }).collect(),
            text: text.to_string(),
            reported: None,
            termios: None,
            background,
        });
        self.touch(id);
        id
    }

    pub fn touch(&mut self, id: usize) {
        self.order.retain(|&j| j != id);
        self.order.push(id);
    }

    pub fn get(&self, id: usize) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn remove(&mut self, id: usize) {
        self.jobs.retain(|j| j.id != id);
        self.order.retain(|&j| j != id);
    }

    /// Current job (`%+`): the most recent one still in the table.
    pub fn current(&self) -> Option<usize> {
        self.order.iter().rev().copied().find(|id| self.get(*id).is_some())
    }

    pub fn previous(&self) -> Option<usize> {
        self.order.iter().rev().copied().filter(|id| self.get(*id).is_some()).nth(1)
    }

    /// Record a wait status for `pid`. Returns the job id it belongs to.
    pub fn update(&mut self, pid: libc::pid_t, ws: WaitStatus) -> Option<usize> {
        for job in &mut self.jobs {
            if let Some(p) = job.procs.iter_mut().find(|p| p.pid == pid) {
                p.state = match ws {
                    WaitStatus::Stopped(sig) => ProcState::Stopped(sig),
                    WaitStatus::Continued => ProcState::Running,
                    other => ProcState::Done(other),
                };
                return Some(job.id);
            }
        }
        None
    }

    /// Resolve a job spec: `%n`, `%+`, `%%`, `%-`, `%string`, `%?string`, or a pgid/pid.
    pub fn resolve(&self, spec: Option<&str>) -> Result<usize, String> {
        let Some(spec) = spec else {
            return self.current().ok_or_else(|| "no current job".to_string());
        };
        let s = spec.strip_prefix('%').unwrap_or(spec);
        let found = match s {
            "" | "+" | "%" => self.current(),
            "-" => self.previous(),
            n if n.chars().all(|c| c.is_ascii_digit()) => {
                let n: i64 = n.parse().unwrap_or(-1);
                if spec.starts_with('%') {
                    self.get(n as usize).map(|j| j.id)
                } else {
                    self.jobs.iter().find(|j| j.procs.iter().any(|p| p.pid as i64 == n)).map(|j| j.id)
                }
            }
            q if q.starts_with('?') => self.jobs.iter().find(|j| j.text.contains(&q[1..])).map(|j| j.id),
            prefix => self.jobs.iter().find(|j| j.text.starts_with(prefix)).map(|j| j.id),
        };
        found.ok_or_else(|| format!("{spec}: no such job"))
    }

    pub fn marker(&self, id: usize) -> char {
        if Some(id) == self.current() {
            '+'
        } else if Some(id) == self.previous() {
            '-'
        } else {
            ' '
        }
    }

    pub fn format(&self, job: &Job, long: bool) -> String {
        let mut line = format!("[{}]{}  ", job.id, self.marker(job.id));
        if long {
            line.push_str(&format!("{} ", job.pgid));
        }
        line.push_str(&format!("{:<24}{}", job.describe_state(), job.text));
        line
    }
}
