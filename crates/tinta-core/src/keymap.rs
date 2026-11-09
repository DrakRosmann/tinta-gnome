//! Named commands and the keys bound to them. The palette, the menus and the keyboard all go
//! through this table, so a command exists in one place.

use serde::Deserialize;
use std::collections::HashMap;

macro_rules! commands {
    ($($variant:ident, $id:literal, $en:literal, $pt:literal, [$($key:literal),*];)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant),*];

            pub fn id(self) -> &'static str {
                match self { $(Command::$variant => $id),* }
            }

            pub fn title(self, portuguese: bool) -> &'static str {
                match self { $(Command::$variant => if portuguese { $pt } else { $en }),* }
            }

            pub fn default_keys(self) -> &'static [&'static str] {
                match self { $(Command::$variant => &[$($key),*]),* }
            }

            pub fn from_id(id: &str) -> Option<Command> {
                match id { $($id => Some(Command::$variant),)* _ => None }
            }
        }
    };
}

commands! {
    // File
    NewFile, "file.new", "New File", "Novo arquivo", ["ctrl+n"];
    OpenFile, "file.open", "Open File…", "Abrir arquivo…", ["ctrl+o"];
    OpenFolder, "file.openFolder", "Open Folder…", "Abrir pasta…", ["ctrl+k ctrl+o"];
    CloseFolder, "file.closeFolder", "Close Folder", "Fechar pasta", [];
    Save, "file.save", "Save", "Salvar", ["ctrl+s"];
    SaveAs, "file.saveAs", "Save As…", "Salvar como…", ["ctrl+shift+s"];
    SaveAll, "file.saveAll", "Save All", "Salvar tudo", ["ctrl+k s"];
    Revert, "file.revert", "Revert File", "Reverter arquivo", [];
    CloseTab, "tab.close", "Close Tab", "Fechar aba", ["ctrl+w"];
    CloseOtherTabs, "tab.closeOthers", "Close Other Tabs", "Fechar outras abas", [];
    CloseAllTabs, "tab.closeAll", "Close All Tabs", "Fechar todas as abas", ["ctrl+k ctrl+w"];
    ReopenTab, "tab.reopen", "Reopen Closed Tab", "Reabrir aba fechada", ["ctrl+shift+t"];
    PinTab, "tab.pin", "Pin/Unpin Tab", "Fixar/desafixar aba", [];
    NextTab, "tab.next", "Next Tab", "Próxima aba", ["ctrl+pagedown", "ctrl+tab"];
    PrevTab, "tab.prev", "Previous Tab", "Aba anterior", ["ctrl+pageup", "ctrl+shift+tab"];
    MoveTabLeft, "tab.moveLeft", "Move Tab Left", "Mover aba para a esquerda", ["ctrl+shift+pageup"];
    MoveTabRight, "tab.moveRight", "Move Tab Right", "Mover aba para a direita", ["ctrl+shift+pagedown"];
    Quit, "app.quit", "Quit", "Sair", ["ctrl+q"];
    // Clipboard and history
    Undo, "edit.undo", "Undo", "Desfazer", ["ctrl+z"];
    Redo, "edit.redo", "Redo", "Refazer", ["ctrl+y", "ctrl+shift+z"];
    Cut, "edit.cut", "Cut", "Recortar", ["ctrl+x", "shift+delete"];
    Copy, "edit.copy", "Copy", "Copiar", ["ctrl+c", "ctrl+insert"];
    Paste, "edit.paste", "Paste", "Colar", ["ctrl+v", "shift+insert"];
    // Lines
    DeleteLine, "line.delete", "Delete Line", "Apagar linha", ["ctrl+shift+k"];
    DuplicateSelection, "edit.duplicate", "Duplicate Selection", "Duplicar seleção", ["ctrl+shift+d"];
    CopyLineDown, "line.copyDown", "Copy Line Down", "Copiar linha abaixo", ["shift+alt+down"];
    CopyLineUp, "line.copyUp", "Copy Line Up", "Copiar linha acima", ["shift+alt+up"];
    MoveLineUp, "line.moveUp", "Move Line Up", "Mover linha para cima", ["alt+up"];
    MoveLineDown, "line.moveDown", "Move Line Down", "Mover linha para baixo", ["alt+down"];
    InsertLineBelow, "line.insertBelow", "Insert Line Below", "Inserir linha abaixo", ["ctrl+enter"];
    InsertLineAbove, "line.insertAbove", "Insert Line Above", "Inserir linha acima", ["ctrl+shift+enter"];
    JoinLines, "line.join", "Join Lines", "Juntar linhas", ["ctrl+shift+j"];
    IndentLines, "line.indent", "Indent Lines", "Indentar linhas", ["ctrl+]"];
    OutdentLines, "line.outdent", "Outdent Lines", "Desindentar linhas", ["ctrl+["];
    SortAsc, "line.sortAsc", "Sort Lines Ascending", "Ordenar linhas (A→Z)", [];
    SortDesc, "line.sortDesc", "Sort Lines Descending", "Ordenar linhas (Z→A)", [];
    SortCaseless, "line.sortCaseless", "Sort Lines Ignoring Case", "Ordenar linhas sem diferenciar maiúsculas", [];
    ReverseLines, "line.reverse", "Reverse Lines", "Inverter ordem das linhas", [];
    UniqueLines, "line.unique", "Remove Duplicate Lines", "Remover linhas duplicadas", [];
    ShuffleLines, "line.shuffle", "Shuffle Lines", "Embaralhar linhas", [];
    TrimTrailing, "edit.trimTrailing", "Trim Trailing Whitespace", "Remover espaços no fim das linhas", [];
    // Comments and case
    ToggleComment, "edit.comment", "Toggle Line Comment", "Comentar/descomentar linha", ["ctrl+/", "ctrl+;"];
    ToggleBlockComment, "edit.blockComment", "Toggle Block Comment", "Comentar/descomentar bloco", ["ctrl+shift+a"];
    UpperCase, "text.upper", "Transform to Uppercase", "Transformar em MAIÚSCULAS", ["ctrl+k ctrl+u"];
    LowerCase, "text.lower", "Transform to Lowercase", "Transformar em minúsculas", ["ctrl+k ctrl+l"];
    TitleCase, "text.title", "Transform to Title Case", "Transformar em Iniciais Maiúsculas", [];
    SnakeCase, "text.snake", "Transform to snake_case", "Transformar em snake_case", [];
    CamelCase, "text.camel", "Transform to camelCase", "Transformar em camelCase", [];
    PascalCase, "text.pascal", "Transform to PascalCase", "Transformar em PascalCase", [];
    KebabCase, "text.kebab", "Transform to kebab-case", "Transformar em kebab-case", [];
    Transpose, "text.transpose", "Transpose Characters", "Trocar caracteres de lugar", ["ctrl+t"];
    DeleteWordLeft, "edit.deleteWordLeft", "Delete Word Left", "Apagar palavra à esquerda", ["ctrl+backspace"];
    DeleteWordRight, "edit.deleteWordRight", "Delete Word Right", "Apagar palavra à direita", ["ctrl+delete"];
    DeleteToLineStart, "edit.deleteToStart", "Delete to Line Start", "Apagar até o início da linha", ["ctrl+shift+backspace"];
    DeleteToLineEnd, "edit.deleteToEnd", "Delete to Line End", "Apagar até o fim da linha", ["ctrl+shift+delete"];
    // Selection
    SelectAll, "select.all", "Select All", "Selecionar tudo", ["ctrl+a"];
    SelectLine, "select.line", "Select Line", "Selecionar linha", ["ctrl+l"];
    SelectWord, "select.word", "Select Word", "Selecionar palavra", [];
    AddNextOccurrence, "select.addNext", "Add Next Occurrence", "Adicionar próxima ocorrência", ["ctrl+d"];
    SkipOccurrence, "select.skip", "Skip Occurrence", "Pular ocorrência", ["ctrl+k ctrl+d"];
    SelectAllOccurrences, "select.allOccurrences", "Select All Occurrences", "Selecionar todas as ocorrências", ["ctrl+shift+l", "ctrl+f2"];
    SplitIntoLines, "select.splitLines", "Add Cursors to Line Ends", "Adicionar cursores ao fim das linhas", ["shift+alt+i"];
    AddCursorAbove, "select.cursorAbove", "Add Cursor Above", "Adicionar cursor acima", ["ctrl+alt+up"];
    AddCursorBelow, "select.cursorBelow", "Add Cursor Below", "Adicionar cursor abaixo", ["ctrl+alt+down"];
    ExpandSelection, "select.expand", "Expand Selection", "Expandir seleção", ["shift+alt+right"];
    ShrinkSelection, "select.shrink", "Shrink Selection", "Reduzir seleção", ["shift+alt+left"];
    // Find
    Find, "find.open", "Find", "Localizar", ["ctrl+f"];
    Replace, "find.replace", "Replace", "Substituir", ["ctrl+h"];
    FindNext, "find.next", "Find Next", "Localizar próximo", ["f3"];
    FindPrev, "find.prev", "Find Previous", "Localizar anterior", ["shift+f3"];
    FindSelection, "find.useSelection", "Find with Selection", "Localizar a seleção", ["ctrl+e"];
    ReplaceAll, "find.replaceAll", "Replace All", "Substituir tudo", ["ctrl+alt+enter"];
    FindInFiles, "search.open", "Find in Files", "Localizar nos arquivos", ["ctrl+shift+f"];
    ReplaceInFiles, "search.replace", "Replace in Files", "Substituir nos arquivos", ["ctrl+shift+h"];
    // Navigation
    CommandPalette, "palette.commands", "Command Palette", "Paleta de comandos", ["ctrl+shift+p", "f1"];
    QuickOpen, "palette.files", "Go to File…", "Ir para arquivo…", ["ctrl+p"];
    GoToLine, "palette.line", "Go to Line…", "Ir para linha…", ["ctrl+g"];
    GoToSymbol, "palette.symbols", "Go to Symbol in File…", "Ir para símbolo no arquivo…", ["ctrl+shift+o", "ctrl+r"];
    ChangeLanguage, "palette.language", "Change Language Mode…", "Mudar linguagem…", ["ctrl+k m"];
    ChangeTheme, "palette.theme", "Color Theme…", "Tema de cores…", ["ctrl+k ctrl+t"];
    OpenRecent, "palette.recent", "Open Recent…", "Abrir recente…", ["ctrl+shift+e"];
    GoBack, "nav.back", "Go Back", "Voltar", ["alt+left"];
    GoForward, "nav.forward", "Go Forward", "Avançar", ["alt+right"];
    JumpToBracket, "nav.bracket", "Go to Matching Bracket", "Ir para o par do parêntese", ["ctrl+shift+\\", "ctrl+m"];
    SelectToBracket, "nav.selectBracket", "Select to Matching Bracket", "Selecionar até o par do parêntese", [];
    ToggleBookmark, "bookmark.toggle", "Toggle Bookmark", "Marcar/desmarcar linha", ["ctrl+f8"];
    NextBookmark, "bookmark.next", "Next Bookmark", "Próximo marcador", ["f8"];
    PrevBookmark, "bookmark.prev", "Previous Bookmark", "Marcador anterior", ["shift+f8"];
    ClearBookmarks, "bookmark.clear", "Clear Bookmarks", "Limpar marcadores", [];
    NextChange, "git.nextChange", "Next Change", "Próxima alteração", ["alt+f3", "f7"];
    PrevChange, "git.prevChange", "Previous Change", "Alteração anterior", ["shift+alt+f3", "shift+f7"];
    RevertHunk, "git.revertHunk", "Revert Change Under Cursor", "Reverter alteração sob o cursor", [];
    // Folding
    Fold, "fold.fold", "Fold", "Dobrar", ["ctrl+shift+["];
    Unfold, "fold.unfold", "Unfold", "Desdobrar", ["ctrl+shift+]"];
    FoldToggle, "fold.toggle", "Toggle Fold", "Dobrar/desdobrar", [];
    FoldAll, "fold.all", "Fold All", "Dobrar tudo", ["ctrl+k ctrl+0"];
    UnfoldAll, "fold.none", "Unfold All", "Desdobrar tudo", ["ctrl+k ctrl+j"];
    // Code intelligence
    TriggerCompletion, "lsp.complete", "Trigger Suggestions", "Mostrar sugestões", ["ctrl+space"];
    ParameterHints, "lsp.signature", "Parameter Hints", "Dicas de parâmetros", ["ctrl+shift+space"];
    ShowHover, "lsp.hover", "Show Hover", "Mostrar informações", ["ctrl+k ctrl+i"];
    GoToDefinition, "lsp.definition", "Go to Definition", "Ir para definição", ["f12"];
    GoToDeclaration, "lsp.declaration", "Go to Declaration", "Ir para declaração", [];
    GoToImplementation, "lsp.implementation", "Go to Implementation", "Ir para implementação", ["ctrl+f12"];
    GoToTypeDefinition, "lsp.typeDefinition", "Go to Type Definition", "Ir para definição do tipo", [];
    FindReferences, "lsp.references", "Find All References", "Localizar referências", ["shift+f12"];
    Rename, "lsp.rename", "Rename Symbol", "Renomear símbolo", ["f2"];
    FormatDocument, "lsp.format", "Format Document", "Formatar documento", ["shift+alt+f"];
    CodeAction, "lsp.codeAction", "Quick Fix…", "Correção rápida…", ["ctrl+."];
    RestartLsp, "lsp.restart", "Restart Language Server", "Reiniciar servidor de linguagem", [];
    NextDiagnostic, "diag.next", "Next Problem", "Próximo problema", ["f4"];
    PrevDiagnostic, "diag.prev", "Previous Problem", "Problema anterior", ["shift+f4"];
    // View
    ToggleSidebar, "view.sidebar", "Toggle Sidebar", "Mostrar/ocultar barra lateral", ["ctrl+b"];
    ShowExplorer, "view.explorer", "Show Explorer", "Mostrar explorador", ["ctrl+shift+x"];
    ShowSearch, "view.search", "Show Search", "Mostrar busca", [];
    ShowGit, "view.git", "Show Source Control", "Mostrar controle de versão", ["ctrl+shift+g"];
    ShowOutline, "view.outline", "Show Outline", "Mostrar estrutura do arquivo", [];
    TogglePanel, "view.panel", "Toggle Bottom Panel", "Mostrar/ocultar painel inferior", ["ctrl+j"];
    ToggleTerminal, "view.terminal", "Toggle Terminal", "Mostrar/ocultar terminal", ["ctrl+`", "ctrl+'"];
    NewTerminal, "terminal.new", "New Terminal", "Novo terminal", ["ctrl+shift+`", "ctrl+shift+'"];
    ShowProblems, "view.problems", "Show Problems", "Mostrar problemas", ["ctrl+shift+m"];
    SplitRight, "view.split", "Split Editor", "Dividir editor", ["ctrl+\\"];
    FocusNextGroup, "view.nextGroup", "Focus Next Editor Group", "Focar próximo grupo de editores", ["ctrl+k ctrl+right"];
    FocusPrevGroup, "view.prevGroup", "Focus Previous Editor Group", "Focar grupo de editores anterior", ["ctrl+k ctrl+left"];
    CloseGroup, "view.closeGroup", "Close Editor Group", "Fechar grupo de editores", [];
    RunFile, "run.file", "Run File", "Executar arquivo", ["ctrl+f5", "ctrl+alt+n"];
    RunWithArgs, "run.withArgs", "Run File with Arguments…", "Executar arquivo com argumentos…", [];
    RunRerun, "run.rerun", "Run Last File Again", "Executar de novo o último arquivo", [];
    RunStop, "run.stop", "Stop Running", "Parar a execução", ["ctrl+alt+m"];
    RunSelection, "run.selection", "Run Selection or Line in REPL", "Executar seleção ou linha no REPL", ["shift+enter"];
    RunOpenRepl, "run.repl", "Open REPL", "Abrir REPL", [];
    RunConfigure, "run.configure", "Edit run.json", "Editar run.json", [];
    TrustFolder, "workspace.trust", "Trust This Folder", "Confiar nesta pasta", [];
    UntrustFolder, "workspace.untrust", "Stop Trusting This Folder", "Deixar de confiar nesta pasta", [];
    DebugStart, "debug.start", "Start or Continue Debugging", "Iniciar ou continuar a depuração", ["f5"];
    DebugStop, "debug.stop", "Stop Debugging", "Parar a depuração", ["shift+f5"];
    DebugRestart, "debug.restart", "Restart Debugging", "Reiniciar a depuração", ["ctrl+shift+f5"];
    DebugPause, "debug.pause", "Pause", "Pausar", [];
    StepOver, "debug.stepOver", "Step Over", "Próxima linha", ["f10"];
    StepInto, "debug.stepIn", "Step Into", "Entrar na função", [];
    StepOut, "debug.stepOut", "Step Out", "Sair da função", ["shift+f11"];
    RunToCursor, "debug.runToCursor", "Run to Cursor", "Executar até o cursor", ["ctrl+f10"];
    ToggleBreakpoint, "debug.breakpoint", "Toggle Breakpoint", "Alternar breakpoint", ["f9"];
    BreakpointCondition, "debug.condition", "Conditional Breakpoint…", "Breakpoint condicional…", ["shift+f9"];
    RemoveBreakpoints, "debug.removeBreakpoints", "Remove All Breakpoints", "Remover todos os breakpoints", [];
    ShowDebug, "view.debug", "Show Run and Debug", "Mostrar execução e depuração", [];
    ShowDebugConsole, "view.debugConsole", "Show Debug Console", "Mostrar console de depuração", ["ctrl+shift+y"];
    DebugConfigure, "debug.configure", "Edit launch.json", "Editar launch.json", [];
    DebugSelectConfig, "debug.select", "Choose Debug Configuration", "Escolher configuração de depuração", [];
    ToggleVim, "view.vim", "Toggle Vim Mode", "Alternar modo Vim", [];
    ToggleWordWrap, "view.wrap", "Toggle Word Wrap", "Alternar quebra de linha", ["alt+z"];
    ToggleMinimap, "view.minimap", "Toggle Minimap", "Alternar minimapa", [];
    ToggleWhitespace, "view.whitespace", "Toggle Whitespace", "Alternar espaços visíveis", [];
    ToggleLineNumbers, "view.lineNumbers", "Toggle Line Numbers", "Alternar números de linha", [];
    ToggleStickyScroll, "view.sticky", "Toggle Sticky Scroll", "Alternar rolagem aderente", [];
    ToggleBlame, "view.blame", "Toggle Inline Git Blame", "Alternar git blame na linha", [];
    ToggleFullscreen, "view.fullscreen", "Toggle Full Screen", "Alternar tela cheia", ["f11"];
    ToggleZen, "view.zen", "Toggle Zen Mode", "Alternar modo zen", ["ctrl+k z"];
    ZoomIn, "view.zoomIn", "Zoom In", "Aumentar zoom", ["ctrl+=", "ctrl++"];
    ZoomOut, "view.zoomOut", "Zoom Out", "Diminuir zoom", ["ctrl+-"];
    ZoomReset, "view.zoomReset", "Reset Zoom", "Restaurar zoom", ["ctrl+0"];
    ScrollUp, "view.scrollUp", "Scroll Up", "Rolar para cima", ["ctrl+up"];
    ScrollDown, "view.scrollDown", "Scroll Down", "Rolar para baixo", ["ctrl+down"];
    CenterCursor, "view.center", "Center Cursor in View", "Centralizar cursor", ["ctrl+k ctrl+c"];
    MarkdownPreview, "view.markdown", "Open Markdown Preview", "Abrir pré-visualização de Markdown", ["ctrl+shift+v"];
    // Git
    GitRefresh, "git.refresh", "Git: Refresh", "Git: atualizar", [];
    GitCommit, "git.commit", "Git: Commit", "Git: commit", [];
    GitPush, "git.push", "Git: Push", "Git: enviar (push)", [];
    GitPull, "git.pull", "Git: Pull", "Git: receber (pull)", [];
    GitFetch, "git.fetch", "Git: Fetch", "Git: buscar (fetch)", [];
    GitStageAll, "git.stageAll", "Git: Stage All", "Git: preparar tudo", [];
    GitLog, "git.log", "Git: Show Log", "Git: mostrar histórico", [];
    GitCheckout, "git.checkout", "Git: Switch Branch…", "Git: trocar de branch…", [];
    GitDiffFile, "git.diff", "Git: Show Changes in File", "Git: mostrar alterações do arquivo", [];
    CompareSaved, "file.compareSaved", "Compare with Saved", "Comparar com o arquivo salvo", [];
    // Tasks
    RunBuild, "task.build", "Run Build Task", "Executar tarefa de build", ["ctrl+shift+b"];
    RunTask, "palette.tasks", "Run Task…", "Executar tarefa…", [];
    RerunTask, "task.rerun", "Rerun Last Task", "Repetir última tarefa", [];
    StopTask, "task.stop", "Stop Task", "Parar tarefa", [];
    ShowTasks, "view.tasks", "Show Task Output", "Mostrar saída das tarefas", [];
    NextTaskProblem, "task.nextProblem", "Next Build Problem", "Próximo problema do build", ["f6"];
    PrevTaskProblem, "task.prevProblem", "Previous Build Problem", "Problema anterior do build", ["shift+f6"];
    // Document
    SetLf, "doc.lf", "Line Endings: LF", "Fim de linha: LF", [];
    SetCrlf, "doc.crlf", "Line Endings: CRLF", "Fim de linha: CRLF", [];
    IndentSpaces, "doc.spaces", "Indent Using Spaces", "Indentar com espaços", [];
    IndentTabs, "doc.tabs", "Indent Using Tabs", "Indentar com tabulações", [];
    ChangeIndentSize, "palette.indent", "Change Indentation Size…", "Mudar tamanho da indentação…", [];
    ChangeEncoding, "palette.encoding", "Reopen/Save with Encoding…", "Reabrir/salvar com codificação…", [];
    InsertSnippet, "palette.snippet", "Insert Snippet…", "Inserir snippet…", [];
    OpenFolderInFiles, "file.reveal", "Reveal in File Manager", "Mostrar no gerenciador de arquivos", [];
    CopyPath, "file.copyPath", "Copy Path of Active File", "Copiar caminho do arquivo", [];
    CopyRelativePath, "file.copyRelPath", "Copy Relative Path", "Copiar caminho relativo", [];
    // App
    OpenSettings, "app.settings", "Settings", "Configurações", ["ctrl+,"];
    EditHeaderBar, "app.editHeaderBar", "Customize Header Bar…", "Editar barra superior…", [];
    OpenSettingsJson, "app.settingsJson", "Open Settings (JSON)", "Abrir configurações (JSON)", [];
    OpenKeybindings, "app.keybindings", "Open Keyboard Shortcuts (JSON)", "Abrir atalhos de teclado (JSON)", [];
    OpenSnippets, "app.snippets", "Open User Snippets (JSON)", "Abrir snippets do usuário (JSON)", [];
    ShowShortcuts, "app.shortcuts", "Keyboard Shortcuts Reference", "Referência de atalhos de teclado", ["ctrl+k ctrl+s"];
    About, "app.about", "About Tinta", "Sobre o Tinta", [];
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stroke {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    /// Lowercase name: `a`, `enter`, `f1`, `pageup`, `/`.
    pub key: String,
}

impl Stroke {
    pub fn parse(s: &str) -> Option<Self> {
        let mut st = Stroke { ctrl: false, shift: false, alt: false, logo: false, key: String::new() };
        let lower = s.trim().to_lowercase();
        if lower.is_empty() {
            return None;
        }
        // "ctrl++" means the plus key: don't split it away.
        let (mods, key) = match lower.strip_suffix("++") {
            Some(m) => (m.to_string(), "+".to_string()),
            None => {
                let i = lower.rfind('+');
                match i {
                    Some(i) => (lower[..i].to_string(), lower[i + 1..].to_string()),
                    None => (String::new(), lower.clone()),
                }
            }
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m {
                "ctrl" | "control" => st.ctrl = true,
                "shift" => st.shift = true,
                "alt" | "option" => st.alt = true,
                "super" | "cmd" | "meta" | "logo" | "win" => st.logo = true,
                _ => return None,
            }
        }
        if key.is_empty() {
            return None;
        }
        st.key = key;
        Some(st)
    }

    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.logo {
            parts.push("Super".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        let mut cs = self.key.chars();
        let key = match self.key.as_str() {
            "pageup" => "PgUp".to_string(),
            "pagedown" => "PgDn".to_string(),
            _ => match cs.next() {
                Some(c) => c.to_uppercase().chain(cs).collect(),
                None => String::new(),
            },
        };
        parts.push(key);
        parts.join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyResult {
    /// Not bound to anything.
    None,
    /// First half of a chord; waiting for the next stroke.
    Pending,
    Run(Command),
}

#[derive(Debug, Clone, Deserialize)]
struct UserBinding {
    key: String,
    command: String,
}

#[derive(Debug, Default, Clone)]
pub struct Keymap {
    single: HashMap<Stroke, Command>,
    chords: HashMap<(Stroke, Stroke), Command>,
    chord_prefixes: HashMap<Stroke, usize>,
    labels: HashMap<Command, String>,
    pending: Option<Stroke>,
}

impl Keymap {
    pub fn with_defaults() -> Self {
        let mut m = Self::default();
        for c in Command::ALL {
            for k in c.default_keys() {
                m.bind(k, *c);
            }
        }
        m
    }

    fn bind(&mut self, keys: &str, cmd: Command) -> bool {
        let mut it = keys.split_whitespace();
        let (Some(a), b) = (it.next().and_then(Stroke::parse), it.next()) else { return false };
        match b {
            None => {
                self.single.insert(a.clone(), cmd);
                self.labels.entry(cmd).or_insert_with(|| a.label());
            }
            Some(b) => {
                let Some(b) = Stroke::parse(b) else { return false };
                let label = format!("{} {}", a.label(), b.label());
                *self.chord_prefixes.entry(a.clone()).or_insert(0) += 1;
                self.chords.insert((a, b), cmd);
                self.labels.entry(cmd).or_insert(label);
            }
        }
        true
    }

    fn unbind(&mut self, keys: &str) {
        let mut it = keys.split_whitespace();
        let (Some(a), b) = (it.next().and_then(Stroke::parse), it.next()) else { return };
        match b.and_then(Stroke::parse) {
            None => {
                if let Some(c) = self.single.remove(&a) {
                    self.labels.remove(&c);
                }
            }
            Some(b) => {
                if let Some(c) = self.chords.remove(&(a.clone(), b)) {
                    self.labels.remove(&c);
                    if let Some(n) = self.chord_prefixes.get_mut(&a) {
                        *n -= 1;
                        if *n == 0 {
                            self.chord_prefixes.remove(&a);
                        }
                    }
                }
            }
        }
    }

    /// Applies a user file: `[{ "key": "ctrl+t", "command": "file.new" }]`, and a command of
    /// `"-"` frees a key. Returns problems found, for the user to see.
    pub fn apply_user(&mut self, src: &str) -> Vec<String> {
        let list: Vec<UserBinding> = match serde_json::from_str(&crate::settings::strip_jsonc(src)) {
            Ok(l) => l,
            Err(e) => return vec![e.to_string()],
        };
        let mut errs = Vec::new();
        for b in list {
            if b.command == "-" {
                self.unbind(&b.key);
                continue;
            }
            match Command::from_id(&b.command) {
                Some(c) => {
                    self.labels.remove(&c);
                    if !self.bind(&b.key, c) {
                        errs.push(format!("invalid key: {}", b.key));
                    }
                }
                None => errs.push(format!("unknown command: {}", b.command)),
            }
        }
        errs
    }

    /// Feeds a stroke through the map, following chords.
    pub fn press(&mut self, stroke: Stroke) -> KeyResult {
        if let Some(first) = self.pending.take() {
            return match self.chords.get(&(first, stroke)) {
                Some(c) => KeyResult::Run(*c),
                None => KeyResult::None,
            };
        }
        if self.chord_prefixes.contains_key(&stroke) {
            self.pending = Some(stroke);
            return KeyResult::Pending;
        }
        match self.single.get(&stroke) {
            Some(c) => KeyResult::Run(*c),
            None => KeyResult::None,
        }
    }

    /// Would this stroke do anything right now? Lets callers try an alternative spelling of a
    /// key first (`ctrl+shift+=` and `ctrl++` are the same keys).
    pub fn would_match(&self, stroke: &Stroke) -> bool {
        match &self.pending {
            Some(first) => self.chords.contains_key(&(first.clone(), stroke.clone())),
            None => self.chord_prefixes.contains_key(stroke) || self.single.contains_key(stroke),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn pending_label(&self) -> Option<String> {
        self.pending.as_ref().map(Stroke::label)
    }

    /// Text for menus and the palette, like `Ctrl+Shift+P`.
    pub fn label(&self, cmd: Command) -> Option<&str> {
        self.labels.get(&cmd).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(k: &str) -> Stroke {
        Stroke::parse(k).unwrap()
    }

    #[test]
    fn parses_strokes() {
        let st = s("ctrl+shift+p");
        assert!(st.ctrl && st.shift && !st.alt);
        assert_eq!(st.key, "p");
        assert_eq!(s("ctrl++").key, "+");
        assert_eq!(s("f1").key, "f1");
        assert!(Stroke::parse("hyper+x").is_none());
        assert_eq!(s("ctrl+shift+p").label(), "Ctrl+Shift+P");
    }

    #[test]
    fn every_default_key_parses_and_ids_are_unique() {
        let mut ids = std::collections::HashSet::new();
        for c in Command::ALL {
            assert!(ids.insert(c.id()), "duplicate id {}", c.id());
            assert_eq!(Command::from_id(c.id()), Some(*c));
            for k in c.default_keys() {
                for part in k.split_whitespace() {
                    assert!(Stroke::parse(part).is_some(), "{k}");
                }
            }
        }
    }

    #[test]
    fn no_default_key_is_bound_twice() {
        let mut seen = std::collections::HashMap::new();
        for c in Command::ALL {
            for k in c.default_keys() {
                if let Some(other) = seen.insert(*k, c.id()) {
                    panic!("{k} bound to both {other} and {}", c.id());
                }
            }
        }
    }

    #[test]
    fn a_chord_prefix_is_not_also_a_single_key() {
        let m = Keymap::with_defaults();
        for prefix in m.chord_prefixes.keys() {
            assert!(!m.single.contains_key(prefix), "{prefix:?}");
        }
    }

    #[test]
    fn chords_wait_for_the_second_stroke() {
        let mut m = Keymap::with_defaults();
        assert_eq!(m.press(s("ctrl+k")), KeyResult::Pending);
        assert!(m.is_pending());
        assert_eq!(m.press(s("ctrl+d")), KeyResult::Run(Command::SkipOccurrence));
        assert!(!m.is_pending());
        assert_eq!(m.press(s("ctrl+k")), KeyResult::Pending);
        assert_eq!(m.press(s("x")), KeyResult::None);
        assert_eq!(m.press(s("ctrl+d")), KeyResult::Run(Command::AddNextOccurrence));
    }

    #[test]
    fn user_bindings_override_and_unbind() {
        let mut m = Keymap::with_defaults();
        let errs = m.apply_user(r#"[
            // comment
            { "key": "ctrl+shift+n", "command": "file.new" },
            { "key": "ctrl+s", "command": "-" },
            { "key": "x", "command": "nope" },
        ]"#);
        assert_eq!(errs.len(), 1);
        assert_eq!(m.press(s("ctrl+shift+n")), KeyResult::Run(Command::NewFile));
        assert_eq!(m.label(Command::NewFile), Some("Ctrl+Shift+N"));
        assert_eq!(m.press(s("ctrl+s")), KeyResult::None);
    }
}
