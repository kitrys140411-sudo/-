//! Syntax tree of a shell program.

/// A word is a sequence of parts; quoting is kept so expansion knows what
/// undergoes field splitting and globbing.
#[derive(Debug, Clone, PartialEq)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, Clone, PartialEq)]
pub enum WordPart {
    /// Unquoted literal text (subject to globbing).
    Literal(String),
    /// Text from single quotes, or escaped with a backslash.
    Quoted(String),
    /// Contents of double quotes.
    DoubleQuoted(Vec<WordPart>),
    /// `~` or `~user` at the start of a word.
    Tilde(String),
    Param(Param),
    /// `$(...)` or backquotes.
    CommandSubst(Box<Program>),
    /// `$((...))`, already expanded as a word before evaluation.
    Arith(Box<Word>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub op: ParamOp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamOp {
    Plain,
    /// `${#name}`
    Length,
    /// `${name:-word}` (colon = also when empty)
    Default { colon: bool, word: Word },
    Assign { colon: bool, word: Word },
    Error { colon: bool, word: Word },
    Alternative { colon: bool, word: Word },
    /// `${name%pat}` / `${name%%pat}`
    RemoveSuffix { longest: bool, pattern: Word },
    RemovePrefix { longest: bool, pattern: Word },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirKind {
    /// `<`
    In,
    /// `>`
    Out,
    /// `>|`
    Clobber,
    /// `>>`
    Append,
    /// `<>`
    ReadWrite,
    /// `<&`
    DupIn,
    /// `>&`
    DupOut,
    /// `<<` / `<<-` (body already collected; `expand` false if the delimiter was quoted)
    HereDoc { body: Word, expand: bool },
    /// `&>` (stdout and stderr to file)
    OutErr,
    /// `&>>`
    AppendOutErr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub fd: Option<i32>,
    pub kind: RedirKind,
    pub target: Word,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseItem {
    pub patterns: Vec<Word>,
    pub body: Program,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Simple(SimpleCommand),
    Group(Program, Vec<Redirect>),
    Subshell(Program, Vec<Redirect>),
    If { branches: Vec<(Program, Program)>, otherwise: Option<Program>, redirects: Vec<Redirect> },
    While { until: bool, cond: Program, body: Program, redirects: Vec<Redirect> },
    For { var: String, items: Option<Vec<Word>>, body: Program, redirects: Vec<Redirect> },
    Case { word: Word, items: Vec<CaseItem>, redirects: Vec<Redirect> },
    Function { name: String, body: Box<Command> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub negate: bool,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(AndOrOp, Pipeline)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub and_or: AndOr,
    pub background: bool,
    /// Source text, used for job listings.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program(pub Vec<ListItem>);
