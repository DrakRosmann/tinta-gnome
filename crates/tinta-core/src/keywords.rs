//! The words each language is made of: its keywords, and the functions, types and macros that come
//! with it. Offered as you type, with or without a language server, in saved files or not.

/// What a word is, for the note beside it in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    /// A function, type, constant or macro the language or its standard library provides.
    Builtin,
}

/// The words of a language, by the name the editor gives it. Empty for one it doesn't know.
pub fn for_language(language: &str) -> Vec<(&'static str, Kind)> {
    let (keywords, builtins): (&[&str], &[&str]) = match language {
        "Rust" => (RUST, RUST_STD),
        "C" => (C, C_STD),
        "C++" => (CPP, CPP_STD),
        "Python" => (PYTHON, PYTHON_STD),
        "JavaScript" | "JavaScript (Babel)" | "JSX" => (JAVASCRIPT, JAVASCRIPT_STD),
        "TypeScript" | "TypeScriptReact" => (TYPESCRIPT, TYPESCRIPT_STD),
        "Go" => (GO, GO_STD),
        "Java" => (JAVA, JAVA_STD),
        "C#" => (CSHARP, CSHARP_STD),
        "Kotlin" => (KOTLIN, KOTLIN_STD),
        "Swift" => (SWIFT, SWIFT_STD),
        "PHP" => (PHP, PHP_STD),
        "Ruby" => (RUBY, RUBY_STD),
        "Lua" => (LUA, LUA_STD),
        "Bourne Again Shell (bash)" => (BASH, BASH_STD),
        "SQL" => (SQL, SQL_STD),
        "HTML" => (&[], HTML),
        "CSS" => (&[], CSS),
        _ => (&[], &[]),
    };
    // C++ is C and more: its own list adds to C's.
    let mut out: Vec<(&str, Kind)> = Vec::new();
    if language == "C++" {
        out.extend(C.iter().map(|w| (*w, Kind::Keyword)));
        out.extend(C_STD.iter().map(|w| (*w, Kind::Builtin)));
    }
    if language.starts_with("TypeScript") {
        out.extend(JAVASCRIPT.iter().map(|w| (*w, Kind::Keyword)));
        out.extend(JAVASCRIPT_STD.iter().map(|w| (*w, Kind::Builtin)));
    }
    out.extend(keywords.iter().map(|w| (*w, Kind::Keyword)));
    out.extend(builtins.iter().map(|w| (*w, Kind::Builtin)));
    let mut seen = std::collections::HashSet::new();
    out.retain(|(w, _)| seen.insert(*w));
    out
}

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop",
    "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "union", "unsafe", "use",
    "where", "while",
];
const RUST_STD: &[&str] = &[
    "bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "String", "Vec", "Option",
    "Some", "None", "Result", "Ok", "Err", "Box", "Rc", "Arc", "RefCell", "Cell", "Mutex", "RwLock", "HashMap", "HashSet", "BTreeMap", "BTreeSet",
    "VecDeque", "Path", "PathBuf", "Clone", "Copy", "Debug", "Default", "Display", "Iterator", "IntoIterator", "From", "Into", "TryFrom", "TryInto",
    "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Send", "Sync", "Sized", "Drop", "Fn", "FnMut", "FnOnce", "AsRef", "ToString", "derive",
    "println!", "print!", "eprintln!", "eprint!", "format!", "vec!", "panic!", "assert!", "assert_eq!", "assert_ne!", "debug_assert!", "unreachable!",
    "todo!", "unimplemented!", "write!", "writeln!", "matches!", "dbg!", "include_str!", "env!", "concat!", "cfg!",
];

const C: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern", "float", "for", "goto", "if", "inline", "int",
    "long", "register", "restrict", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void",
    "volatile", "while", "bool", "true", "false", "include", "define", "ifdef", "ifndef", "endif", "pragma", "undef",
];
const C_STD: &[&str] = &[
    "NULL", "EOF", "FILE", "size_t", "stdin", "stdout", "stderr", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
    "printf", "scanf", "fprintf", "fscanf", "sprintf", "snprintf", "sscanf", "puts", "fgets", "fputs", "putchar", "getchar", "fopen", "fclose", "fread",
    "fwrite", "fseek", "ftell", "fflush", "perror", "malloc", "calloc", "realloc", "free", "memcpy", "memset", "memmove", "memcmp", "strlen", "strcpy",
    "strncpy", "strcat", "strncat", "strcmp", "strncmp", "strchr", "strrchr", "strstr", "strtok", "strdup", "atoi", "atof", "strtol", "strtod", "exit",
    "abs", "rand", "srand", "time", "qsort", "bsearch", "sqrt", "pow", "sin", "cos", "tan", "floor", "ceil", "fabs", "isdigit", "isalpha", "isspace",
    "toupper", "tolower", "assert",
];

const CPP: &[&str] = &[
    "alignas", "alignof", "catch", "class", "concept", "consteval", "constexpr", "constinit", "const_cast", "co_await", "co_return", "co_yield", "decltype",
    "delete", "dynamic_cast", "explicit", "export", "final", "friend", "mutable", "namespace", "new", "noexcept", "nullptr", "operator", "override",
    "private", "protected", "public", "reinterpret_cast", "requires", "static_assert", "static_cast", "template", "this", "thread_local", "throw", "try",
    "typeid", "typename", "using", "virtual",
];
const CPP_STD: &[&str] = &[
    "std", "string", "string_view", "vector", "array", "map", "unordered_map", "set", "unordered_set", "deque", "list", "pair", "tuple", "optional",
    "variant", "unique_ptr", "shared_ptr", "weak_ptr", "make_unique", "make_shared", "make_pair", "cout", "cin", "cerr", "endl", "getline", "move",
    "forward", "swap", "sort", "find", "begin", "end", "size", "push_back", "emplace_back", "to_string", "stoi", "function", "thread", "mutex",
];

const PYTHON: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return", "try",
    "while", "with", "yield",
];
const PYTHON_STD: &[&str] = &[
    "print", "input", "len", "range", "enumerate", "zip", "map", "filter", "sorted", "reversed", "sum", "min", "max", "abs", "round", "int", "float",
    "str", "bool", "list", "dict", "set", "tuple", "frozenset", "bytes", "type", "isinstance", "issubclass", "hasattr", "getattr", "setattr", "open",
    "iter", "next", "any", "all", "repr", "format", "id", "hash", "super", "object", "property", "staticmethod", "classmethod", "self", "Exception",
    "ValueError", "TypeError", "KeyError", "IndexError", "RuntimeError", "StopIteration", "FileNotFoundError", "NotImplementedError", "__init__",
    "__name__", "__main__", "__str__", "__repr__",
];

const JAVASCRIPT: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "false",
    "finally", "for", "function", "get", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "set", "static", "super", "switch",
    "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "yield",
];
const JAVASCRIPT_STD: &[&str] = &[
    "console", "log", "Math", "JSON", "Object", "Array", "String", "Number", "Boolean", "Promise", "Map", "Set", "Date", "RegExp", "Error", "Symbol",
    "parseInt", "parseFloat", "isNaN", "setTimeout", "setInterval", "clearTimeout", "clearInterval", "fetch", "document", "window", "require",
    "module", "exports", "length", "push", "forEach", "filter", "reduce", "includes", "querySelector", "addEventListener",
];
const TYPESCRIPT: &[&str] = &[
    "abstract", "any", "as", "bigint", "boolean", "declare", "enum", "implements", "infer", "interface", "is", "keyof", "namespace", "never", "number",
    "object", "private", "protected", "public", "readonly", "satisfies", "string", "symbol", "type", "unknown",
];
const TYPESCRIPT_STD: &[&str] = &["Record", "Partial", "Required", "Readonly", "Pick", "Omit", "Exclude", "Extract", "ReturnType", "Awaited", "NonNullable"];

const GO: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface",
    "map", "package", "range", "return", "select", "struct", "switch", "type", "var",
];
const GO_STD: &[&str] = &[
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max", "min", "new", "panic", "print", "println", "real",
    "recover", "bool", "byte", "rune", "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "float32",
    "float64", "complex64", "complex128", "string", "error", "any", "comparable", "true", "false", "nil", "iota", "fmt", "Println", "Printf",
    "Sprintf", "Errorf",
];

const JAVA: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "continue", "default", "do", "double", "else", "enum", "extends",
    "final", "finally", "float", "for", "if", "implements", "import", "instanceof", "int", "interface", "long", "native", "new", "package", "permits",
    "private", "protected", "public", "record", "return", "sealed", "short", "static", "super", "switch", "synchronized", "this", "throw", "throws",
    "transient", "try", "var", "void", "volatile", "while", "yield", "true", "false", "null",
];
const JAVA_STD: &[&str] = &[
    "String", "System", "out", "println", "printf", "Math", "Integer", "Double", "Long", "Boolean", "Character", "Object", "List", "ArrayList",
    "LinkedList", "Map", "HashMap", "Set", "HashSet", "Arrays", "Collections", "Scanner", "Optional", "Exception", "RuntimeException", "Override",
    "StringBuilder", "Thread",
];

const CSHARP: &[&str] = &[
    "abstract", "as", "async", "await", "base", "bool", "break", "byte", "case", "catch", "char", "checked", "class", "const", "continue", "decimal",
    "default", "delegate", "do", "double", "else", "enum", "event", "explicit", "extern", "false", "finally", "fixed", "float", "for", "foreach", "get",
    "if", "implicit", "in", "init", "int", "interface", "internal", "is", "lock", "long", "namespace", "new", "null", "object", "operator", "out",
    "override", "params", "private", "protected", "public", "readonly", "record", "ref", "return", "sealed", "set", "short", "sizeof", "static",
    "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "uint", "ulong", "unchecked", "unsafe", "ushort", "using", "var", "virtual",
    "void", "volatile", "when", "where", "while", "yield",
];
const CSHARP_STD: &[&str] = &["Console", "WriteLine", "ReadLine", "Math", "List", "Dictionary", "HashSet", "String", "Task", "Exception", "Linq", "Select", "Where"];

const KOTLIN: &[&str] = &[
    "abstract", "as", "break", "by", "catch", "class", "companion", "const", "continue", "data", "do", "else", "enum", "false", "final", "finally", "for",
    "fun", "if", "import", "in", "init", "inline", "interface", "internal", "is", "lateinit", "null", "object", "open", "override", "package", "private",
    "protected", "public", "return", "sealed", "super", "suspend", "this", "throw", "true", "try", "typealias", "val", "var", "when", "while",
];
const KOTLIN_STD: &[&str] = &[
    "println", "print", "readLine", "listOf", "mutableListOf", "mapOf", "mutableMapOf", "setOf", "arrayOf", "String", "Int", "Long", "Double", "Boolean",
    "Unit", "Any", "Nothing", "List", "Map", "let", "apply", "also", "run", "with", "lazy",
];

const SWIFT: &[&str] = &[
    "associatedtype", "async", "await", "break", "case", "catch", "class", "continue", "default", "defer", "deinit", "do", "else", "enum", "extension",
    "fallthrough", "false", "fileprivate", "for", "func", "guard", "if", "import", "in", "init", "inout", "internal", "is", "let", "nil", "open",
    "operator", "private", "protocol", "public", "repeat", "rethrows", "return", "self", "Self", "static", "struct", "subscript", "super", "switch",
    "throw", "throws", "true", "try", "typealias", "var", "where", "while",
];
const SWIFT_STD: &[&str] = &["print", "String", "Int", "Double", "Bool", "Array", "Dictionary", "Set", "Optional", "Character", "readLine", "map", "filter"];

const PHP: &[&str] = &[
    "abstract", "and", "array", "as", "break", "case", "catch", "class", "clone", "const", "continue", "declare", "default", "do", "echo", "else",
    "elseif", "empty", "enum", "extends", "final", "finally", "fn", "for", "foreach", "function", "global", "if", "implements", "include", "instanceof",
    "interface", "isset", "list", "match", "namespace", "new", "or", "print", "private", "protected", "public", "readonly", "require", "require_once",
    "return", "static", "switch", "throw", "trait", "try", "unset", "use", "var", "while", "yield", "true", "false", "null",
];
const PHP_STD: &[&str] = &[
    "strlen", "str_replace", "substr", "strpos", "explode", "implode", "count", "array_map", "array_filter", "array_merge", "in_array", "json_encode",
    "json_decode", "var_dump", "print_r", "sprintf", "intval",
];

const RUBY: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module",
    "next", "nil", "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then", "true", "undef", "unless", "until", "when", "while", "yield",
];
const RUBY_STD: &[&str] = &["puts", "print", "gets", "require", "attr_accessor", "attr_reader", "each", "map", "select", "Array", "Hash", "String", "Integer"];

const LUA: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return",
    "then", "true", "until", "while",
];
const LUA_STD: &[&str] = &[
    "print", "pairs", "ipairs", "type", "tostring", "tonumber", "require", "pcall", "error", "assert", "setmetatable", "getmetatable", "select", "table",
    "string", "math", "io", "os",
];

const BASH: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do", "done", "in", "function", "time", "return", "exit",
    "break", "continue", "local", "export", "readonly", "declare", "unset", "shift", "source", "alias", "set", "trap", "eval", "exec", "wait",
];
const BASH_STD: &[&str] = &[
    "echo", "printf", "read", "cd", "pwd", "test", "ls", "cat", "grep", "sed", "awk", "find", "sort", "uniq", "head", "tail", "cut", "tr", "xargs",
    "mkdir", "rm", "cp", "mv", "chmod", "touch", "basename", "dirname", "true", "false",
];

const SQL: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "DROP", "ALTER", "ADD", "INDEX", "VIEW", "JOIN",
    "INNER", "LEFT", "RIGHT", "OUTER", "ON", "AS", "AND", "OR", "NOT", "NULL", "IS", "IN", "LIKE", "BETWEEN", "ORDER", "BY", "GROUP", "HAVING",
    "LIMIT", "OFFSET", "DISTINCT", "UNION", "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "DEFAULT", "UNIQUE", "CASE", "WHEN", "THEN", "ELSE", "END",
];
const SQL_STD: &[&str] = &["COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "INTEGER", "TEXT", "VARCHAR", "BOOLEAN", "DATE", "TIMESTAMP", "NOW"];

const HTML: &[&str] = &[
    "html", "head", "body", "title", "meta", "link", "script", "style", "div", "span", "p", "a", "img", "ul", "ol", "li", "table", "tr", "td", "th",
    "thead", "tbody", "form", "input", "button", "label", "select", "option", "textarea", "header", "footer", "nav", "main", "section", "article",
    "aside", "h1", "h2", "h3", "h4", "h5", "h6", "strong", "em", "code", "pre", "br", "hr", "class", "id", "href", "src", "alt", "type", "name",
    "value",
];
const CSS: &[&str] = &[
    "color", "background", "background-color", "border", "border-radius", "margin", "padding", "width", "height", "max-width", "min-height", "display",
    "flex", "grid", "position", "top", "left", "right", "bottom", "font-family", "font-size", "font-weight", "line-height", "text-align", "justify-content",
    "align-items", "gap", "overflow", "opacity", "transition", "transform", "z-index", "box-shadow", "cursor", "none", "block", "inline", "relative",
    "absolute", "fixed", "auto", "important",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_named_here_is_one_the_editor_has() {
        let names: Vec<String> = crate::language::all().iter().map(|l| l.name.clone()).collect();
        for n in [
            "Rust", "C", "C++", "Python", "JavaScript", "TypeScript", "Go", "Java", "C#", "Kotlin", "Swift", "PHP", "Ruby", "Lua", "Bourne Again Shell (bash)",
            "SQL", "HTML", "CSS",
        ] {
            assert!(names.iter().any(|x| x == n), "{n} isn't a language name; the names are {names:?}");
            assert!(!for_language(n).is_empty(), "{n}");
        }
    }

    #[test]
    fn cpp_has_c_and_no_word_twice() {
        let cpp = for_language("C++");
        assert!(cpp.iter().any(|(w, _)| *w == "printf"));
        assert!(cpp.iter().any(|(w, _)| *w == "nullptr"));
        let mut words: Vec<_> = cpp.iter().map(|(w, _)| *w).collect();
        let n = words.len();
        words.sort();
        words.dedup();
        assert_eq!(words.len(), n);
        assert!(for_language("Plain Text").is_empty());
    }
}
