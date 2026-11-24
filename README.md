# VILauncher

Launcher para Linux que executa um jogo de Xbox Series X|S (GDK) **que você possui**,
usando uma das duas camadas de compatibilidade deste projeto:

| Backend | Repositório | O que é |
|---|---|---|
| `victory` | [VILinux](https://github.com/Isaachi1/VILinux) (VIctory) | runtime específico de Grand Theft Auto VI |
| `xcl` | [LinuxCL](https://github.com/Isaachi1/LinuxCL) (XCL) | camada genérica para títulos GDK (Minecraft Preview, Fortnite, …) |

## Como funciona (e o que "nativo" quer dizer aqui)

VIctory e XCL são programas **Windows** (`victory.exe`, `xcl.exe`): carregam o executável do
console no próprio processo, implementam as bibliotecas do console (`kernelx`, XGameRuntime,
D3D12.X, …) e recompilam os shaders RDNA2 para DXIL. Nenhum dos dois compila para Linux.

O que eles já têm é suporte explícito a Wine: os dois detectam Wine (`wine_get_version`) e mudam
de comportamento nele, e o código D3D12 do XCL foi escrito contra o vkd3d-proton. Então, no Linux,
o caminho é:

```
jogo (x86-64 do console) ─► victory.exe / xcl.exe ─► Wine ou Proton ─► vkd3d-proton (D3D12 → Vulkan) ─► GPU
```

O código x86-64 do jogo continua rodando direto na CPU, sem emulação. O VILauncher monta essa
cadeia: acha o Proton (ou usa o Wine do sistema com vkd3d-proton), prepara o prefixo, converte os
caminhos Linux para os que o programa vê dentro do Wine (`/home/eu/jogo` → `Z:\home\eu\jogo`),
passa as variáveis que cada backend documenta e verifica a máquina antes de rodar.

**O que ele não faz:** não baixa, não desempacota e não descriptografa pacotes (XVC / MSIXVC), não
contorna DRM nem licenças, e não inclui nada do jogo ou do SDK do console. Ele só roda a pasta
desempacotada de um jogo seu. Uma pasta com um pacote é recusada.

## Estado

- O launcher em si está testado (33 testes, com Proton, Wine, jogo e backend simulados).
- **Não foi testado ainda com um Wine/Proton real, uma GPU e um jogo.** O VIctory diz no README dele
  que o jogo ainda **não é jogável** nem no Windows (o mundo não renderiza direito e pode travar a
  GPU). Rodar via Wine adiciona as diferenças do vkd3d-proton por cima disso.

## Requisitos

- Linux x86-64, CPU com AVX2, Python 3.11+.
- GPU com driver Vulkan com `VK_EXT_mesh_shader` (Mesa RADV recente ou NVIDIA). Os dois backends
  precisam de shader model 6.6 e mesh shaders, o que só o vkd3d-proton oferece (o `d3d12` do próprio
  Wine não).
- **Proton** (GE-Proton ou Proton Experimental, instalado pelo Steam), **ou** Wine 9+ mais uma
  release do [vkd3d-proton](https://github.com/HansKristian-Work/vkd3d-proton/releases) descompactada.
- O build Windows do backend:
  - `victory.exe`: `cargo build --release -p victory` no VILinux (num Windows; o toolchain fixado é
    `x86_64-pc-windows-msvc`).
  - `xcl.exe`: `cargo build --release` no LinuxCL, mais o DXC
    ([DirectXShaderCompiler releases](https://github.com/microsoft/DirectXShaderCompiler/releases),
    `bin/x64` com `dxcompiler.dll` e `dxil.dll`) e um `opus.dll` (libopus para Windows; sem ele o
    áudio Opus fica mudo no Wine).
- Memória: o XCL mediu ~15 GB comprometidos nos menus de um título recente; 32 GB (RAM + swap) é
  o mais seguro.

Ajustes do sistema que o `doctor` sugere quando faltam:

```sh
sudo sysctl -w vm.max_map_count=1048576          # títulos grandes mapeiam muitas regiões
sudo sysctl -w vm.unprivileged_userfaultfd=1     # o vkd3d-proton precisa ver as escritas na memória do jogo (XCL #175)
```

## Uso

```sh
pip install --user .            # ou: python3 -m vilauncher ...

# Proton (detectado automaticamente) + VIctory
vilauncher setup --game ~/Jogos/gta6 --backend-exe ~/builds/victory.exe

# ou Wine do sistema + vkd3d-proton + XCL
vilauncher setup --game ~/Jogos/titulo --backend-exe ~/builds/xcl/xcl.exe \
    --runner wine --vkd3d ~/Downloads/vkd3d-proton-2.14 --dxc-dir ~/dxc/bin/x64 --opus-dir ~/opus

vilauncher doctor               # verifica máquina e configuração
vilauncher run --dry-run        # mostra o comando e o ambiente, sem rodar
vilauncher run                  # roda
vilauncher run -- -argumento    # argumentos para o jogo
vilauncher desktop              # cria a entrada no menu de aplicativos
```

Outros comandos: `vilauncher info [pasta]` (o que o `MicrosoftGame.config` diz), `vilauncher protons`
(os Protons encontrados), `vilauncher config` (a configuração atual).

### Configuração

`~/.config/vilauncher/config.toml` (ou `$VILAUNCHER_CONFIG`). O `setup` escreve; dá para editar:

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

[env]
VICTORY_LOG = "info"          # qualquer variável extra para a execução
```

Onde ficam os arquivos:

| O quê | Onde |
|---|---|
| prefixo do Proton / Wine | `~/.local/share/vilauncher/compatdata` / `prefix` |
| saves, configurações e logs do jogo | `~/.local/share/vilauncher/<backend>/data` (`VICTORY_DATA_DIR`, `XCL_DATA_ROOT`) |
| cache de shaders e dados convertidos | `~/.cache/vilauncher/<backend>` (`VICTORY_CACHE_DIR`) |
| saída da última execução pelo menu | `~/.local/state/vilauncher/last-run.log` |

Como o VIctory, o launcher recusa pastas de dados, cache ou jogo dentro da árvore do VILinux
(`.victory-root`): nada derivado do jogo vai parar num repositório.

## Testes

```sh
python3 -m unittest -v
```

Os testes usam um Proton falso (um script que grava como foi chamado), um prefixo Wine falso e
cabeçalhos PE gerados pelos próprios testes. Nenhum dado de jogo.

## Licença

GPL-3.0-or-later (`LICENSE`), como o VILinux.
