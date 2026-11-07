# Tinta para GNOME e COSMIC

Um fork do [Tinta](https://github.com/DrakRosmann/cosmic-ext-tinta), o editor de código feito com o
libcosmic, que **toma a cara do ambiente onde roda**: no COSMIC continua igual ao original; no GNOME
usa o visual do GNOME.

![No GNOME: cores do Adwaita, título no meio do cabeçalho e o menu atrás de um botão](assets/screenshot-gnome.png)

![O menu principal, à direita, como nos aplicativos do GNOME](assets/screenshot-gnome-menu.png)

## O que muda no GNOME

Continua sendo o mesmo aplicativo, feito com o libcosmic, e não um aplicativo GTK. O que ele faz é
ler as preferências do GNOME e se vestir com elas:

- **Cores:** as superfícies do libadwaita (fundo, barra lateral, menus), claro ou escuro conforme o
  GNOME, e a **cor de destaque** escolhida nas configurações do sistema
- **Formas:** os cantos do Adwaita, 6 nos botões e 12 nos menus
- **Fontes:** a da interface e a de largura fixa do GNOME (Adwaita Sans e Adwaita Mono, por
  padrão); uma fonte escolhida nas configurações do editor continua valendo
- **Ícones:** o tema de ícones do GNOME
- **Cabeçalho:** o título no meio, todos os menus atrás de um botão no fim, e os botões de
  minimizar e maximizar só se o GNOME estiver configurado para mostrá-los
- **Ao vivo:** trocar o modo escuro, a cor de destaque ou a fonte de largura fixa no GNOME muda o
  editor aberto, sem reiniciar

Ele decide pelo `XDG_CURRENT_DESKTOP`. Em outros ambientes (KDE, Sway) fica com o visual do COSMIC.
Nas configurações, "Estilo da interface" deixa escolher: seguir o ambiente, COSMIC ou GNOME. Voltar do
GNOME para o COSMIC com o editor aberto troca as cores na hora; fontes e ícones, ao reiniciar.

**O que não muda:** os controles continuam sendo os do libcosmic. Botões, listas, menus e caixas
de texto têm as cores e os cantos do GNOME, mas não o comportamento exato do GTK. Os botões da
janela ficam sempre à direita, mesmo que o GNOME esteja configurado para colocá-los à esquerda.

Este fork usa nome e pastas próprios (`tinta-gnome`, `~/.config/tinta-gnome`), então pode ser
instalado ao lado do Tinta original.

![Tinta com a árvore de arquivos, o editor e o terminal](assets/screenshot.png)

![Sugestões e problemas vindos do servidor de linguagem](assets/screenshot-lsp.png)

![Depurando um programa em C: variáveis, pilha de chamadas e o que mudou em destaque](assets/screenshot-debug.png)

![Diff lado a lado, com o trecho alterado em destaque](assets/screenshot-diff.png)

![O tema claro, com o editor dividido](assets/screenshot-light.png)

## O que ele faz, igual ao original

**Edição**

- Múltiplos cursores: `Ctrl+D` na próxima ocorrência, `Ctrl+Shift+L` em todas, `Alt+clique`,
  `Ctrl+Alt+↑/↓`, seleção em coluna com `Shift+Alt+arrastar` ou o botão do meio
- Fecha parênteses e aspas sozinho, digita por cima do que fechou, envolve a seleção
- Indentação automática, detecção de espaços ou tabs, comentar linha e bloco
- Mover, duplicar, juntar, ordenar, inverter, embaralhar e tirar duplicatas de linhas
- Maiúsculas, minúsculas, `snake_case`, `camelCase`, `PascalCase`, `kebab-case`
- Expandir a seleção (palavra → dentro dos parênteses → com os parênteses → linha → tudo)
- Desfazer e refazer por agrupamento, como nos editores que você já conhece
- Snippets com pontos de parada, embutidos para as linguagens mais comuns e seus próprios em JSON
- Teclas mortas e métodos de entrada: `á`, `ç`, `ã` no ABNT2 funcionam, CJK também
- Modo Vim, nas configurações ou em "Alternar modo Vim" (veja abaixo)

**Visualização**

- Destaque de sintaxe para 220 gramáticas, incremental: digitar no meio de um arquivo de
  300 mil linhas reprocessa só o que mudou
- 13 temas de cores, além do COSMIC, que acompanha o claro e o escuro do sistema
- Parênteses coloridos por profundidade, ignorando strings e comentários
- Minimapa, guias de indentação, rolagem aderente, réguas, espaços visíveis
- Dobra de código, quebra de linha, números de linha absolutos ou relativos, a partir da árvore de
  sintaxe (tree-sitter) em Rust, Python, C, C++, JavaScript, TypeScript, Go, Java, Bash, JSON, HTML
  e CSS, e da indentação nas outras linguagens
- Destaque do par do parêntese e das ocorrências da palavra sob o cursor
- Breadcrumbs, divisão em vários grupos, modo zen, tela cheia
- Pré-visualização de Markdown ao lado do texto

**Navegação**

- Paleta de comandos (`Ctrl+Shift+P`) com busca aproximada e todos os comandos
- `Ctrl+P` para arquivos; digitando `>` vira comandos, `:` vira linha, `@` vira símbolo
- Voltar e avançar, marcadores, próximo problema, próxima alteração do git
- Estrutura do arquivo na barra lateral

**Busca**

- Localizar e substituir com expressão regular, palavra inteira, maiúsculas, dentro da seleção
- Busca na pasta inteira, respeitando o `.gitignore`, com filtros de arquivos e substituição, que
  alcança todas as ocorrências mesmo quando a lista na tela para em um limite

**Inteligência de código (LSP)**

Usa os servidores de linguagem que estiverem instalados, sem configuração: rust-analyzer, clangd,
pyright, gopls, typescript-language-server, lua-language-server e os de mais de vinte outras
linguagens.

- Sugestões ao digitar, misturadas com palavras do arquivo e snippets
- Informações ao passar o mouse, dicas de parâmetros
- Ir para definição, declaração, implementação e tipo; localizar referências
- Renomear símbolo em todos os arquivos, formatar, correções rápidas
- Problemas sublinhados, com a mensagem no fim da linha e um painel com todos
- Envio parcial: o servidor recebe só o trecho alterado, se aceitar isso

**Depurador (DAP)**

`F5` compila e roda, e pronto. Usa o `gdb` 14 ou mais novo, o `lldb-dap`, o `debugpy` ou o `dlv`,
os que estiverem instalados. Os testes rodam contra programas de verdade no gdb 15, 16, 17 e 18 e no
debugpy. O gdb 14 é aceito, mas não foi testado, e o lldb-dap e o dlv usam o mesmo código, mas ainda
não foram experimentados.

- Breakpoints com um clique à esquerda dos números de linha ou `F9`, condicionais com `Shift+F9`.
  Acompanham o código quando o arquivo muda e continuam lá da próxima vez
- Variáveis em árvore, com o que mudou desde a última parada em destaque, e expressões observadas
- Pilha de chamadas e threads: clicar em um nível mostra aquele nível
- Passar o mouse sobre um nome, com o programa pausado, mostra o valor
- Console com a saída do programa, que avalia expressões; `-exec` manda um comando ao depurador
- Um erro de compilação aparece no código como qualquer outro, e a sessão não começa

Sem configuração, ele compila o arquivo aberto (C e C++), usa `cargo`, `make` ou `cmake`, ou roda o
arquivo (Python e Go). Para o resto, ou para passar argumentos, há o `.tinta/launch.json`, que aceita
também o formato do VS Code. O comando "Editar launch.json" cria um modelo:

```jsonc
[
  {
    "name": "Servidor",
    "type": "gdb",                 // gdb, lldb, python ou go
    "build": "make -j8",           // opcional: a sessão só começa se der certo
    "program": "${workspaceFolder}/build/servidor",
    "args": ["--porta", "8080"],
    "cwd": "${workspaceFolder}",
    "env": { "LOG": "debug" },
    "stopOnEntry": false
  }
]
```

**Tarefas de build**

- `Ctrl+Shift+B` roda o build; a paleta lista o que o projeto oferece: cargo, make, just, cmake,
  meson, go, npm, pytest e o seu `.tinta/tasks.json`
- Os erros aparecem no código e no painel de problemas, e `F6` e `Shift+F6` pulam de um para o outro
- No terminal, `Ctrl+clique` abre `arquivo:linha` e links

**Git**

- Marcas na margem e no mapa da barra de rolagem, próxima alteração, reverter um trecho
- Preparar, descartar, commit, push, pull, fetch e troca de branch
- Diff lado a lado, com rolagem conjunta, cor de sintaxe e o trecho alterado em destaque. `F7` e
  `Shift+F7` pulam entre as alterações, e um clique duplo no lado novo abre o arquivo naquela linha
- Histórico e `git blame` na linha atual

**E o resto**

- Terminal integrado (o emulador do Alacritty), com vários terminais, seleção e cores
- Árvore de arquivos com menu de contexto, ícones do tema e cores do git
- Sessão restaurada ao abrir, e texto não salvo guardado entre execuções (o _hot exit_ do VS Code)
- Uma janela só: `tinta-gnome arquivo` manda o arquivo para a janela que já está aberta
- Salvamento atômico, salvar automaticamente, formatar ao salvar, limpar espaços no fim das linhas
- Codificações (UTF-8, UTF-16, Windows-1252, Shift JIS…) e fins de linha detectados e preservados
- Recarrega arquivos que mudaram no disco e pergunta se você tem alterações não salvas
- Abrir arquivos arrastando, ou na linha de comando: `tinta-gnome src/main.rs:10:4`
- Interface em português e inglês, conforme o idioma do sistema

## Instalação

```sh
git clone https://github.com/DrakRosmann/tinta-gnome
cd tinta-gnome
./install.sh          # instala em ~/.local
```

O `deps.sh` instala o que faltar com `apt`, `dnf` ou `pacman`. Depois é abrir "Tinta" no lançador,
ou rodar `tinta-gnome [arquivo ou pasta]`.

Para ter inteligência de código, instale o servidor da sua linguagem: `rustup component add
rust-analyzer`, `dnf install clang-tools-extra` e assim por diante. Ele é encontrado no `PATH`.

## Configuração

Tudo pode ser mudado na janela de configurações (`Ctrl+,`) ou à mão, em `~/.config/tinta-gnome/`. O
arquivo é relido ao salvar, sem reiniciar, e aceita comentários e vírgulas sobrando:

```jsonc
// settings.json
{
  "fontFamily": "JetBrains Mono",
  "fontSize": 15,
  "tabSize": 4,
  "wordWrap": false,
  "rulers": [80, 100],
  "formatOnSave": true,
  "syntaxTheme": "COSMIC",
  // Outro servidor para uma linguagem; "command" vazio desliga.
  "lspServers": {
    "python": { "command": "ruff", "args": ["server"] }
  }
}
```

```jsonc
// keybindings.json: "-" libera uma tecla
[
  { "key": "ctrl+shift+n", "command": "file.new" },
  { "key": "ctrl+k ctrl+x", "command": "edit.trimTrailing" },
  { "key": "ctrl+t", "command": "-" }
]
```

```jsonc
// snippets.json, por linguagem
{
  "Rust": [
    { "prefix": "pr", "description": "println", "body": "println!(\"$1\");$0" }
  ]
}
```

A lista completa de atalhos está em `Ctrl+K Ctrl+S`, e os identificadores dos comandos em
[`keymap.rs`](crates/tinta-core/src/keymap.rs).

### Atalhos principais

| Atalho | Faz |
| --- | --- |
| `Ctrl+Shift+P` / `F1` | Paleta de comandos |
| `Ctrl+P` | Ir para arquivo |
| `Ctrl+G` | Ir para linha |
| `Ctrl+Shift+O` | Ir para símbolo |
| `Ctrl+F` / `Ctrl+H` | Localizar / substituir |
| `Ctrl+Shift+F` | Localizar nos arquivos |
| `Ctrl+D` | Próxima ocorrência como outro cursor |
| `Ctrl+Shift+L` | Todas as ocorrências |
| `Alt+↑` / `Alt+↓` | Mover linha |
| `Ctrl+Shift+D` | Duplicar |
| `Ctrl+Shift+K` | Apagar linha |
| `Ctrl+/` ou `Ctrl+;` | Comentar |
| `Ctrl+\` | Dividir o editor |
| `Ctrl+B` | Barra lateral |
| `Ctrl+J` / `` Ctrl+` `` / `Ctrl+'` | Painel / terminal |
| `F12` / `Ctrl+clique` | Ir para definição |
| `F2` | Renomear símbolo |
| `Ctrl+.` | Correção rápida |
| `Ctrl+Espaço` | Sugestões |
| `Alt+Z` | Quebra de linha |
| `Ctrl+Shift+V` | Pré-visualizar Markdown |
| `Ctrl+Shift+B` | Rodar o build |
| `F5` / `Shift+F5` | Iniciar ou continuar / parar a depuração |
| `F9` / `Shift+F9` | Breakpoint / breakpoint condicional |
| `F10` / `F11` / `Shift+F11` | Próxima linha / entrar na função / sair dela |
| `F7` / `Shift+F7` | Próxima alteração / anterior |
| `Ctrl+Shift+Y` | Console de depuração |

`F11` é tela cheia, exceto durante uma sessão de depuração. "Ordenar linhas" não tem mais atalho:
`F9` agora é o breakpoint.

### Modo Vim

Os modos normal, inserir, substituir e visual (de caracteres, de linhas e em bloco), com contagens:

- Movimentos: `h j k l w b e W B E ge 0 ^ $ gg G f F t T ; , % { } H M L * # n N`, marcas
- Operadores: `d c y > < gu gU g~ gc`, dobrados para a linha inteira
- Objetos de texto: `iw aw iW aW` e `i`/`a` com `( [ { < " ' \` p`
- `x X s S D C Y p P r R J gJ ~ u Ctrl+R . Ctrl+A Ctrl+X`
- Registradores (`"a`, `"0`, `"_`, e `"+` para a área de transferência) e macros (`q`, `@`, `@@`)
- `/`, `?` e os comandos `:w :q :wq :x :qa :e :bd :sp :vs :tabn :tabp :noh :set :sort :d :N`, e
  `:s` com intervalo (`:%s/a/b/g`, `:'<,'>s/a/b/`), usando a sintaxe de expressões do Vim
- `gd gr gi gh` vão para a inteligência de código, `zc zo za zM zR` para as dobras

No modo inserir quem digita é o próprio editor, com sugestões e fechamento de parênteses. Os atalhos
com `Ctrl` que o Vim não usa, como `Ctrl+P` e `Ctrl+S`, continuam valendo.

## Como é feito

O projeto é dividido para que quase tudo possa ser testado sem abrir janela:

- **`crates/tinta-core`**: o motor, sem nenhuma dependência de interface. Texto em
  [ropey](https://github.com/cessen/ropey), cursores, desfazer, cores sobre as gramáticas do
  [syntect](https://github.com/trishume/syntect), estrutura com
  [tree-sitter](https://tree-sitter.github.io/), busca, git (pelo comando `git`), clientes LSP e DAP,
  o modo Vim, snippets, atalhos.
- **`crates/tinta`**: a interface em [libcosmic](https://github.com/pop-os/libcosmic). O texto é um
  widget próprio, desenhado em uma grade de caracteres para repintar sem refazer o layout. O resto
  (abas, menus, paleta, configurações, diálogos) são os widgets do próprio COSMIC.
- **`crates/tinta-vkbd`**: digita em um compositor pelo protocolo de teclado virtual do Wayland,
  para os testes de ponta a ponta.
- **`crates/tinta-xptr`**: ponteiro real e capturas da tela real, para os mesmos testes.

## Testes

```sh
cargo test                                     # núcleo e interface, sem janela
cargo test --release large_file -- --ignored --nocapture   # mede um arquivo de 300 mil linhas
```

Os testes da interface montam a árvore real de widgets e passam por ela eventos de mouse e teclado,
desenhando cada quadro em um renderizador de software. O cliente LSP é testado contra o clangd de
verdade quando ele está instalado, e o depurador contra o gdb e o debugpy, compilando e rodando
programas pequenos. Quem não tem essas ferramentas vê esses testes passarem sem fazer nada.

Para ver o aplicativo rodando, `tests/e2e/run.sh` abre um `cosmic-comp` aninhado, com configuração
própria, e dirige o app por um script que também tira capturas de tela:

```sh
cargo build --workspace
cat > /tmp/passos.txt <<FIM
wait 500
cmd palette.commands
input salvar
shot /tmp/paleta.png
quit
FIM
tests/e2e/run.sh /tmp/passos.txt
```

Os passos estão descritos em [`script.rs`](crates/tinta/src/app/script.rs).

`tests/e2e/gnome.sh` roda o editor dentro de um **GNOME Shell de verdade**, sem tela, num D-Bus
próprio e com as preferências do GNOME num arquivo próprio, nunca nas suas:

```sh
tests/e2e/gnome.sh /tmp/passos.txt color-scheme=prefer-dark accent-color=teal
```

`tests/e2e/pointer.sh` faz o mesmo com um **ponteiro de verdade**: o compositor roda dentro de um
Xwayland próprio, e o `tinta-xptr` move, clica e captura os pixels da tela, sem mexer no cursor do
desktop. Foi assim que apareceram bugs que nenhum outro teste pegava, como clicar num item de menu
fechar o aplicativo.

## Licença

GPL-3.0-only. COSMIC é uma marca registrada da System76; este é um projeto independente.

## O que falta

- Sem extensões nem edição remota.
- No depurador: sem anexar a um processo em execução, mudar o valor de uma variável, "rodar até o
  cursor" ou breakpoints que só registram no console, e o programa depurado não recebe o que se digita
  (a entrada padrão). Para isso, use o terminal.
- No Vim: sem `:global`, `:normal`, mapeamentos próprios nem `.vimrc`.
- Sem navegação pelo teclado na árvore de arquivos, e arrastar abas ainda não reordena: use
  `Ctrl+Shift+PgUp` e `Ctrl+Shift+PgDn`.
- A quebra de linha fica desligada em arquivos acima de 4 milhões de caracteres.
