use std::{
    collections::HashMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use ariadne::{Color, Config, Label, Report, ReportKind, Source};

use crate::{
    compile::CompileError,
    function::FunctionId,
    lex::{CodeSpan, Sp, Span},
    parse::ParseError,
};

#[derive(Debug)]
pub enum UiuaError {
    Load(PathBuf, io::Error),
    Format(PathBuf, io::Error),
    Compile(Vec<Sp<CompileError>>),
    Run {
        error: Box<RuntimeError>,
        trace: Vec<TraceFrame>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceFrame {
    pub id: FunctionId,
    pub span: Span,
}

#[derive(Debug)]
pub enum RuntimeError {
    Run(Sp<String>),
    Import(UiuaError),
}

impl From<Sp<String>> for RuntimeError {
    fn from(value: Sp<String>) -> Self {
        Self::Run(value)
    }
}

pub type RuntimeResult<T = ()> = Result<T, RuntimeError>;

impl fmt::Display for UiuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiuaError::Load(path, e) => {
                write!(f, "failed to load {}: {e}", path.to_string_lossy())
            }
            UiuaError::Format(path, e) => {
                write!(f, "failed to format {}: {e}", path.to_string_lossy())
            }
            UiuaError::Compile(errors) => {
                for error in errors {
                    writeln!(f, "{error}")?;
                }
                Ok(())
            }
            UiuaError::Run { error, trace } => match &**error {
                RuntimeError::Run(error) => {
                    match &error.span {
                        Span::Code(span) => write!(f, "Error at {}: {}", span, error.value)?,
                        Span::Builtin => write!(f, "Error: {}", error.value)?,
                    }
                    format_trace(f, trace)
                }
                RuntimeError::Import(error) => write!(f, "Import {error}"),
            },
        }
    }
}

impl UiuaError {
    pub fn message(&self) -> String {
        match self {
            UiuaError::Run { error, .. } => error.message(),
            error => error.to_string(),
        }
    }
}

impl RuntimeError {
    pub fn message(&self) -> String {
        match self {
            RuntimeError::Run(error) => error.value.clone(),
            RuntimeError::Import(error) => error.message(),
        }
    }
}

fn format_trace<F: fmt::Write>(f: &mut F, trace: &[TraceFrame]) -> fmt::Result {
    let last = TraceFrame {
        id: FunctionId::Named("".into()),
        span: Span::Builtin,
    };
    let mut last = &last;
    let mut repetitions = 1;
    let max_id_length = trace
        .iter()
        .map(|frame| frame.id.to_string().len())
        .max()
        .unwrap_or(0);
    let max_span_length = trace
        .iter()
        .map(|frame| match &frame.span {
            Span::Code(span) => span.to_string().len(),
            Span::Builtin => 0,
        })
        .max()
        .unwrap_or(0);
    for frame in trace {
        if frame == last {
            repetitions += 1;
        } else {
            if repetitions > 1 {
                writeln!(f, " (x {repetitions})")?;
                repetitions = 0;
            } else {
                writeln!(f)?;
            }
            match &frame.span {
                Span::Code(span) => write!(
                    f,
                    "  in {:max_id_length$} at {:max_span_length$}",
                    frame.id.to_string(),
                    span
                )?,
                Span::Builtin => write!(f, "  in {:max_id_length$}", frame.id.to_string())?,
            }
            last = frame;
        }
    }
    if repetitions > 1 {
        write!(f, " (x {repetitions})")?;
    }
    Ok(())
}

impl From<Vec<Sp<CompileError>>> for UiuaError {
    fn from(errors: Vec<Sp<CompileError>>) -> Self {
        Self::Compile(errors)
    }
}

impl From<Vec<Sp<ParseError>>> for UiuaError {
    fn from(errors: Vec<Sp<ParseError>>) -> Self {
        Self::Compile(errors.into_iter().map(Sp::map_into).collect())
    }
}

impl Error for UiuaError {}

pub type UiuaResult<T = ()> = Result<T, UiuaError>;

impl UiuaError {
    pub fn show(&self, complex_output: bool) -> String {
        let config = Config::default()
            .with_color(complex_output)
            .with_multiline_arrows(false);
        let color = if complex_output {
            Color::Red
        } else {
            Color::Unset
        };
        match self {
            UiuaError::Compile(errors) => {
                if errors.is_empty() {
                    return String::new();
                }
                let mut buffer = Vec::new();
                let mut chache = None;
                for error in errors {
                    if let Span::Code(span) = &error.span {
                        let cache = chache.get_or_insert_with(|| Cache {
                            input: Source::from(&span.input),
                            files: HashMap::new(),
                        });
                        let report = Report::<CodeSpan>::build(
                            ReportKind::Error,
                            span.file.clone(),
                            span.start.pos,
                        )
                        .with_message(&error.value)
                        .with_label(Label::new(span.clone()).with_color(color))
                        .with_config(config)
                        .finish();
                        let _ = report.write(cache, &mut buffer);
                    }
                }
                String::from_utf8_lossy(&buffer).into_owned()
            }
            UiuaError::Run { error, trace } => match &**error {
                RuntimeError::Run(error) => {
                    let mut buffer = Vec::new();
                    if let Span::Code(span) = &error.span {
                        let mut cache = Cache {
                            input: Source::from(&span.input),
                            files: HashMap::new(),
                        };
                        let report = Report::<CodeSpan>::build(
                            ReportKind::Error,
                            span.file.clone(),
                            span.start.pos,
                        )
                        .with_message(&error.value)
                        .with_label(Label::new(span.clone()).with_color(color))
                        .with_config(config)
                        .finish();
                        let _ = report.write(&mut cache, &mut buffer);
                    }
                    let mut s = String::from_utf8_lossy(&buffer).into_owned();
                    format_trace(&mut s, trace).unwrap();
                    s
                }
                RuntimeError::Import(error) => error.show(complex_output),
            },
            _ => self.to_string(),
        }
    }
}

type SourceId = Option<Arc<Path>>;

impl ariadne::Span for CodeSpan {
    type SourceId = SourceId;
    fn source(&self) -> &Self::SourceId {
        &self.file
    }
    fn start(&self) -> usize {
        self.start.pos
    }
    fn end(&self) -> usize {
        self.end.pos
    }
}

struct Cache {
    input: Source,
    files: HashMap<SourceId, Source>,
}

impl ariadne::Cache<SourceId> for Cache {
    fn fetch(&mut self, id: &SourceId) -> Result<&Source, Box<dyn fmt::Debug + '_>> {
        match id {
            Some(path) => {
                if !self.files.contains_key(id) {
                    let text =
                        fs::read_to_string(path).map_err(|e| Box::new(e) as Box<dyn fmt::Debug>)?;
                    let source = Source::from(text);
                    self.files.insert(id.clone(), source);
                }
                Ok(&self.files[id])
            }
            None => Ok(&self.input),
        }
    }
    fn display<'a>(&self, id: &'a SourceId) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(match id {
            Some(path) => Box::new(path.to_string_lossy()),
            None => Box::<String>::default(),
        })
    }
}
