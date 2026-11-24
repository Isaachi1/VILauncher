# VILauncher

Launcher para Linux, em **Rust + Tauri**, que executa um jogo de Xbox Series X|S (GDK) **que você
possui** usando uma das duas camadas de compatibilidade deste projeto:

| Backend | Repositório | O que é |
|---|---|---|
| `victory` | [VIctory](https://github.com/LPortVI/VIctory) | runtime específico de Grand Theft Auto VI |
| `xcl` | [xcl](https://github.com/LPortVI/xcl) (XCL) | camada genérica para títulos GDK (Minecraft Preview, Fortnite, …) |

## Como funciona (e o que "nativo" quer dizer aqui)

VIctory e XCL são programas **Windows** (`victory.exe`, `xcl.exe`). Eles carregam o executável do
console no próprio processo, implementam as bibliotecas do console (`kernelx`, XGameRuntime,
D3D12.X, …) e recompilam os shaders RDNA2 para DXIL. Nenhum dos dois compila para Linux.

O que eles já têm é suporte explícito ao Wine: os dois detectam o Wine (`wine_get_version`) e
mudam de comportamento nele, e o código D3D12 do XCL foi escrito pensando no vkd3d-proton. Então,
no Linux, o caminho é este:

```
jogo (x86-64 do console) ─► victory.exe / xcl.exe ─► Proton ou Wine ─► vkd3d-proton (D3D12 → Vulkan) ─► GPU
```

O código x86-64 do jogo continua rodando direto na CPU, sem emulação. O VILauncher monta essa
cadeia:

- acha o Proton, ou usa o Wine do sistema com o vkd3d-proton copiado para o prefixo;
- prepara o prefixo;
- converte os caminhos Linux para os que o programa enxerga dentro do Wine
  (`/home/eu/jogo` → `Z:\home\eu\jogo`);
- passa as variáveis que cada backend documenta;
- verifica a máquina antes de rodar.

**O que ele não faz:** não baixa, não desempacota e não descriptografa pacotes (XVC / MSIXVC), não
contorna DRM nem licenças, e não inclui nada do jogo ou do SDK do console. Ele só roda a pasta
desempacotada de um jogo seu, e recusa uma pasta que tenha um pacote.

## Estrutura

| Pasta | O quê |
|---|---|
| `crates/core` | `vilauncher-core`: configuração, pasta do jogo, Proton/Wine, backends, verificações, execução |
| `crates/cli` | `vilauncher`: a linha de comando (scripts, atalho do menu, SSH) |
| `app` | `vilauncher-app`: a janela Tauri 2 |
| `ui` | a interface da janela (HTML/CSS/JS sem build; tudo roda no Rust) |
| `ui/fonts` | Oswald (SIL Open Font License, `ui/fonts/OFL.txt`) |

O visual é inspirado no estilo do menu e do material de divulgação do GTA VI: um pôr do sol de
Vice City (azul-cobalto, magenta, rosa e laranja) com palmeiras, abas no estilo do menu de pausa e
tipografia condensada em caixa alta. Tudo é desenho próprio, em CSS e SVG. O launcher não inclui
logo, arte, fonte ou imagem da Rockstar.

A janela tem cinco abas:

- **Jogo:** a pasta do jogo, o executável e os argumentos.
- **Compatibilidade:** o backend, o Proton ou o Wine, o vkd3d-proton, o DXC e o Opus.
- **Verificação:** as checagens da máquina e da configuração, e o comando exato que vai rodar.
- **Log:** a saída do jogo ao vivo, gravada também em `~/.local/state/vilauncher/last-run.log`.
- **Status:** onde cada projeto está (VIctory, XCL e os outros), segundo o
  [PortWatch VI](https://portwatch-vi.xyz). O launcher só lê o `data/tracker.json` do site (a última
  cópia fica em `~/.cache/vilauncher/portwatch-tracker.json` para quando estiver sem rede); não baixa
  nem roda nada dos projetos listados. No terminal: `vilauncher status`.

Atalhos: `Ctrl+Enter` joga, `Ctrl+D` verifica e `Ctrl+1` a `Ctrl+5` trocam de aba. O botão **Jogar** se recusa a rodar enquanto alguma checagem da configuração falha. Dá para forçar
mesmo assim.

## Estado

- O núcleo tem 33 testes (`cargo test --workspace`), que usam um Proton simulado, um prefixo Wine
  simulado e cabeçalhos PE gerados pelos próprios testes.
- A janela foi aberta num X virtual. Nesse teste, verificar, mostrar o comando, jogar (com um Proton
  simulado) e o log ao vivo funcionaram.
- **Ainda não foi testado com um Wine/Proton real, uma GPU e um jogo.** O README do VIctory diz
  que o jogo ainda **não é jogável** nem no Windows: o mundo não renderiza direito e pode travar a
  GPU. Rodar pelo Wine adiciona por cima disso as diferenças do vkd3d-proton.

## Requisitos

- Linux x86-64 e CPU com AVX2.
- GPU com driver Vulkan com `VK_EXT_mesh_shader` (Mesa RADV recente ou NVIDIA). Os dois backends
  precisam de shader model 6.6 e de mesh shaders, e só o vkd3d-proton oferece isso (o `d3d12` do
  próprio Wine não).
- **Proton** (GE-Proton ou Proton Experimental, instalado pelo Steam) **ou** Wine 9+ mais uma
  release do [vkd3d-proton](https://github.com/HansKristian-Work/vkd3d-proton/releases) descompactada.
- O build Windows do backend:
  - `victory.exe`: `cargo build --release -p victory` no VIctory, feito num Windows (o toolchain
    fixado é `x86_64-pc-windows-msvc`).
  - `xcl.exe`: `cargo build --release` no xcl, mais o DXC
    ([DirectXShaderCompiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases),
    pasta `bin/x64` com `dxcompiler.dll` e `dxil.dll`) e um `opus.dll` (libopus para Windows; sem ele
    o áudio Opus fica mudo no Wine).
- Memória: o XCL mediu ~15 GB comprometidos nos menus de um título recente, então 32 GB (RAM + swap)
  é o mais seguro.

Quando faltar, a verificação sugere estes ajustes do sistema:

```sh
sudo sysctl -w vm.max_map_count=1048576          # títulos grandes mapeiam muitas regiões
sudo sysctl -w vm.unprivileged_userfaultfd=1     # o vkd3d-proton precisa ver as escritas na memória do jogo (XCL #175)
```

## Compilar

As dependências do Tauri no Debian/Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev build-essential pkg-config
```

Para compilar e testar:

```sh
cargo build --release --workspace     # target/release/vilauncher-app (janela) e target/release/vilauncher (CLI)
cargo test --workspace
```

Pacotes `.deb` / AppImage: `cargo install tauri-cli --version "^2"` e depois `cargo tauri build`
dentro de `app/`.

### Build pronto (CI)

O workflow `build` (`.github/workflows/build.yml`) compila os três programas: o VILauncher no
Linux e o `xcl.exe` e o `victory.exe` em runners Windows (os dois são programas Windows, que no
Linux rodam pelo Proton/Wine). O `xcl.exe` e o `victory.exe` vêm dos projetos mais avançados com
código público segundo o PortWatch VI (veja abaixo). Ele roda a cada push na `main`, em cada PR,
toda noite (para pegar commits novos e um novo líder no tracker) e à mão, em **Actions › build › Run
workflow**, onde um branch, tag ou commit informado compila o LPortVI/xcl ou o LPortVI/VIctory
naquele ponto em vez da escolha do tracker. Uma tag `v*` publica tudo como release.

O artefato `vilauncher-bundle` traz o `.deb`, o AppImage e `vilauncher-linux-x64.tar.gz`:

```
vilauncher-linux-x64/
├── vilauncher, vilauncher-app        # CLI e janela
├── BUILD.txt                         # os commits usados
└── backends/
    ├── xcl/      xcl.exe, dxcompiler.dll, dxil.dll, opus.dll, licenses/
    └── victory/  victory.exe, licenses/
```

O `xcl.exe` procura o DXC e o `opus.dll` na própria pasta, então para o XCL basta apontar o
executável do backend para `backends/xcl/xcl.exe`; `dxc_dir` e `opus_dir` podem ficar vazios.

### Atualizar o runtime: o port mais avançado do PortWatch VI

Toda noite (e a cada push na `main`), o workflow `build`:

1. **escolhe** (`scripts/select-runtimes.sh`): lê o `tracker.json` do
   [PortWatch VI](https://portwatch-vi.xyz), ordena os projetos com código público pelo marco mais
   avançado (como na aba Status) e, para cada família que ele sabe compilar, pega o primeiro: uma
   árvore do VIctory (`apps/victory/Cargo.toml`) ou do XCL (`crates/xcl/Cargo.toml`), no commit mais
   novo do branch padrão. Repositórios que sumiram (404 / DMCA), arquivados, sem código público ou
   de outro tipo são pulados, com o motivo. Sem nenhum, a família volta para o fork do LPortVI;
2. **compila** esses commits em runners Windows (os dois são programas Windows/MSVC, então o
   launcher não compila nada na sua máquina);
3. **verifica no VirusTotal** (`scripts/virustotal-scan.py`, com o segredo `VT_API_KEY` do
   repositório, num runner Linux): cada executável da build é consultado pelo SHA-256 e, se o
   VirusTotal não o conhece, enviado para análise. Os executáveis são reconhecidos pelo conteúdo,
   não pelo nome, então os tipos mais usados entram: PE do Windows (`.exe`, `.dll`, `.sys`), ELF do
   Linux (programas, `.so`, AppImage), pacotes `.deb`, instaladores MSI, Mach-O e scripts com `#!`.
   O resultado vira um JSON com o repositório e o commit daquela build. Toda noite os arquivos do
   próprio VILauncher (CLI, janela, `.deb` e AppImage) também são verificados, só para registro;
4. **publica** na release `runtimes`: um `.tar.gz` por backend, o JSON do VirusTotal e o
   `runtimes.json` (repositório, commit, tamanho, SHA-256, resultado do VirusTotal, qual família está
   mais avançada e como cada projeto foi escolhido ou pulado). Uma build que o VirusTotal acusa
   **não** é publicada: a anterior daquele backend continua.

Em **Compatibilidade › Procurar builds novas** (ou `vilauncher update`), o launcher mostra de qual
projeto e commit vem cada build e o resultado do VirusTotal. **Instalar e usar** (`vilauncher update
best`, ou `xcl` / `victory`) baixa só dessa release, confere o tamanho e o SHA-256, confere que os
binários são os mesmos que o VirusTotal analisou, recusa uma build acusada, descompacta numa pasta
à parte, só então troca a build em `~/.local/share/vilauncher/runtimes/<backend>` e passa a usá-la.
Depois é só **Jogar**. Se algo falhar, a build anterior fica como estava.

O código de terceiros só roda nos jobs de build do CI, que não têm permissão de escrita, e o que
chega na sua máquina é o binário que passou pelo VirusTotal. Ainda assim, um antivírus não prova que
um programa é inofensivo: os projetos são escolhidos pelo ranking de um site independente.

Para ativar o VirusTotal no CI, crie uma conta gratuita no VirusTotal e salve a chave da API como
segredo `VT_API_KEY` em **Settings › Secrets and variables › Actions**. Sem ela, as builds são
publicadas como "não verificadas".

A chave gratuita permite 4 consultas por minuto, 500 por dia e 15,5 mil por mês, e só para uso não
comercial. O CI respeita isso: consulta pelo hash antes de enviar qualquer arquivo, espaça as
consultas em 16 s, faz no máximo 60 por execução, consulta uma análise em andamento uma vez por
minuto por até 10 minutos e espera um minuto quando recebe 429. Um backend cujo repositório e commit
já estão publicados não é escaneado de novo. Na prática, uma noite com os dois backends novos gasta
umas 30 consultas (mais umas 20 para os arquivos do launcher), e uma noite sem mudanças não gasta
nenhuma.

### VirusTotal na sua máquina (opcional)

Com uma chave salva em **Compatibilidade** (ou `vilauncher virustotal --key CHAVE`; fica em
`~/.config/vilauncher/virustotal.key`, só legível por você, ou em `VILAUNCHER_VT_KEY`), o launcher
também consulta de novo cada build antes de instalar e pode verificar a build em uso, mesmo uma
compilada à mão; os executáveis da pasta são reconhecidos pelo conteúdo, como no CI (**Verificar build atual no VirusTotal**, `vilauncher virustotal`). Só o hash sai da
máquina, a não ser que você peça **Enviar para análise** (`vilauncher virustotal --upload xcl.exe`).
Só os binários da pasta do backend são consultados ou enviados, nunca a pasta do jogo.

## Linha de comando

```sh
# Proton (detectado automaticamente) + VIctory
vilauncher setup --game ~/Jogos/gta6 --backend-exe ~/builds/victory.exe

# ou Wine do sistema + vkd3d-proton + XCL
vilauncher setup --game ~/Jogos/titulo --backend-exe ~/builds/xcl/xcl.exe \
    --runner wine --vkd3d ~/Downloads/vkd3d-proton-2.14 --dxc-dir ~/dxc/bin/x64 --opus-dir ~/opus

vilauncher doctor               # verifica a máquina e a configuração
vilauncher run --dry-run        # mostra o comando e o ambiente, sem rodar
vilauncher run                  # roda
vilauncher run -- -argumento    # argumentos para o jogo
vilauncher desktop              # cria a entrada no menu de aplicativos
vilauncher status               # onde cada projeto está, segundo o PortWatch VI
vilauncher update [best|xcl|victory]  # mostra / instala a build mais nova (best: a mais avançada)
vilauncher virustotal           # consulta o backend em uso no VirusTotal
```

## Configuração

Fica em `~/.config/vilauncher/config.toml` (ou em `$VILAUNCHER_CONFIG`). A janela e o CLI leem e
escrevem o mesmo arquivo:

```toml
[game]
dir = "/home/eu/Jogos/gta6"
args = []

[backend]
kind = "victory"              # ou "xcl"
exe = "/home/eu/builds/victory.exe"
console = "series-x"          # victory: ou "series-s"
unimplemented = "trap"        # victory: ou "zero"

[runner]
kind = "proton"               # ou "wine"
path = "/home/eu/.steam/root/compatibilitytools.d/GE-Proton10-1"

[dirs]

[env]
VICTORY_LOG = "info"          # qualquer variável extra para a execução
```

Onde ficam os arquivos:

| O quê | Onde |
|---|---|
| prefixo do Proton / Wine | `~/.local/share/vilauncher/compatdata` / `prefix` |
| saves, configurações e logs do jogo | `~/.local/share/vilauncher/<backend>/data` (`VICTORY_DATA_DIR`, `XCL_DATA_ROOT`) |
| cache de shaders e dados convertidos | `~/.cache/vilauncher/<backend>` (`VICTORY_CACHE_DIR`) |
| saída da última execução | `~/.local/state/vilauncher/last-run.log` |
| builds instaladas pelo launcher | `~/.local/share/vilauncher/runtimes/<backend>` |

Como o VIctory, o launcher recusa pastas de dados, de cache ou do jogo que fiquem dentro da árvore
do VIctory (`.victory-root`), para que nada derivado do jogo vá parar num repositório.

## Licença

GPL-3.0-or-later (`LICENSE`), como o VIctory.
