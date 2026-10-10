<div align="center">

# 🌐 rembrowser

**A vim-native, privacy-first Chromium browser — written in Rust.**

⌨️ Vim keys everywhere · 🎬 Netflix/Spotify DRM · 🛡️ Built-in ad blocking · 🧘 Zero clutter

</div>

---

## ✨ What it does

| | |
|---|---|
| ⌨️ **Vim-native** | `j k gg G`, hints with `f`, `/` search, counts, maps — in every tab *and* web app |
| 💬 **Command popup** | `:` opens a centred, noice-style popup with fuzzy completion |
| 🎬 **DRM works** | Netflix, Prime, Spotify, Hotstar (Widevine installs itself, no Chrome needed) |
| 🛡️ **Ad & tracker blocking** | uBlock-style lists built in, plus element hiding |
| 🕵️ **Private by default** | no Google sign-in, no sync, no telemetry pings, 3rd-party cookies blocked |
| 🧘 **Zen mode** | no bars — the tab bar and status line pop up only when something happens |
| 🏠 **Homescreen** | clock, search box and your own site tiles (add / remove) |
| 📱 **Web apps** | any site as its own window: `rembrowser --app=https://web.whatsapp.com` |
| 📺 **Picture-in-picture** | `gp` floats the video |
| ✍️ **Edit fields in nvim** | `Ctrl-T` in any text box |
| 📜 **Userscripts** | Greasemonkey-style scripts instead of extensions |
| ⚙️ **YAML config** | one file, applies the moment you save it |

---

## 🚀 Install

### One line (any x86_64 Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/nganaremba-rem/rembrowser/main/install.sh | bash
```

- 🐧 **Arch & Arch-based** → builds the native package (system Chromium engine, every codec). `pacman` installs all dependencies.
- 📦 **Ubuntu 22.04+, Debian 12+, Fedora 35+, …** → installs the portable bundle into `~/.local` (no root). *Note: the portable engine has no H.264, so H.264-only sites won't play.*

### By hand (Arch)

```bash
git clone https://github.com/nganaremba-rem/rembrowser
cd rembrowser/packaging/arch && makepkg -si
```

### Update / remove

```bash
rembrowser update              # engine + system packages, rembrowser, DRM, ad-block lists
rembrowser uninstall           # keeps your profile
rembrowser uninstall --purge   # removes everything
```

---

## ⌨️ Shortcuts

> 💡 Press **`<Space>sk`** inside rembrowser to search all of them.

### 🧭 Moving around
| Key | Action |
|---|---|
| `j` / `k` / `h` / `l` | scroll |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `Ctrl-f` / `Ctrl-b`, `Space` | page down / up |
| `gg` / `G` | top / bottom (`5j` = 5 times) |
| `Ctrl-o` / `Ctrl-i` | back / forward |
| `gu` / `gU` | up one URL level / site root |
| `r` / `R` | reload / reload without cache |

### 🔗 Hints (type the letters shown)
| Key | Action |
|---|---|
| `f` / `F` | click / open in new tab |
| `;y` / `;Y` | yank link / yank text |
| `;i` / `;I` | open image / in new tab |
| `;s` | download link |
| `;e` | edit field in nvim |
| `;k` | remove element |
| `;p` / `;P` | add to read-later queue |

### 🗂️ Tabs
| Key | Action |
|---|---|
| `o` / `t` | open URL here / in new tab |
| `O` / `T` | same, pre-filled with current URL |
| `gt` / `gT`, `Ctrl-Tab` | next / previous tab |
| `Ctrl-t` / `Ctrl-w` | new tab / close tab |
| `u` | reopen closed tab |
| `<Space><Space>` | tab picker |

### 🔍 Search & clipboard
| Key | Action |
|---|---|
| `/` / `?` | search (live highlight + count) |
| `n` / `N` | next / previous match |
| `y` | copy page URL |
| `p` / `P` | open URL from clipboard / in new tab |

### 🔎 Zoom
| Key | Action |
|---|---|
| `Ctrl` `+` / `zi` | zoom in |
| `Ctrl` `-` / `zo` | zoom out |
| `Ctrl` `0` / `zz` | reset — remembered per site |

### 🧰 Leader (`Space`) menu
| Key | Action |
|---|---|
| `<Space>s.` | 🕘 history |
| `<Space>sm` | 🔖 bookmarks (`<Space>ba` adds one) |
| `<Space>sd` | ⬇️ downloads |
| `<Space>sk` | ⌨️ all shortcuts |
| `<Space>ma` / `<Space>mm` / `<Space>1-4` | 📌 harpoon: pin / list / jump |
| `<Space>.` / `<Space>S` | 📚 read later: add / open oldest |
| `<Space>ut` / `<Space>ul` | 📌 pin tab bar / status line |
| `<Space>uc` | ⚙️ edit config |

### 🎛️ Modes & extras
| Key | Action |
|---|---|
| `i` / `gi` | focus a text field (INSERT) |
| `Esc` | back to NORMAL |
| `Ctrl-z` | pass-through: the site gets every key |
| `Ctrl-t` *(in a field)* | edit the field in nvim |
| `gp` | picture-in-picture |
| `gf` / `gF` | page source / DevTools |
| `Ctrl-q` | quit |

### 💬 Inside the `:` popup
`Tab` / `Ctrl-n` next · `Shift-Tab` / `Ctrl-p` previous · `Ctrl-r %` paste URL · `Ctrl-f` edit in nvim · `Esc` close

---

## 💬 Commands

| Command | |
|---|---|
| `:open aw niri` | search with a shortcut (`dd` `g` `gh` `aw` `yt` `w`) |
| `:tabs` `:history` `:bookmarks` `:downloads` `:keys` | pickers |
| `:app` / `:app-install` | turn this page into a web app / add it to your launcher |
| `:pip` | picture-in-picture |
| `:set dark-mode!` | toggle any setting |
| `:config` | edit your config |
| `:userscripts` | reload userscripts |

---

## ⚙️ Config

`rembrowser config` (or `:config`) opens `~/.config/rembrowser/config.yaml` — saving applies it instantly.

```yaml
settings:
  home-page: home          # "home" = the homescreen, or any URL
  status-bar: false        # true = always visible
  tab-bar: false
  dark-mode: true
  download-path: ~/Downloads

search:
  default: dd
  engines:
    aw: https://wiki.archlinux.org/index.php?search=$0

keys:
  normal:
    H: gT
    L: gt
    "<Space>sb": { run: ":tabs<CR>", desc: tabs picker }
```

🔄 Coming from **vimb**? Add `source: [~/.config/vimb/config]` — your vimb config just works.

### 📜 Userscripts
Drop `*.js` files in `~/.config/rembrowser/userscripts/`:

```js
// ==UserScript==
// @match   *://*.youtube.com/*
// ==/UserScript==
GM_addStyle('#comments { display: none !important }');
```

---

## ❓ FAQ

**🧩 Chrome extensions?** Not possible with this engine mode (CEF's Alloy runtime) — use userscripts. Ad blocking is built in.

**⚙️ `chrome://settings`?** Doesn't exist here. Use `:config` / `:set`. `chrome://version` and `chrome://gpu` still work.

**🎬 DRM didn't play on first launch?** Widevine downloads in the background on first run — restart once.

**📱 Web apps keep their own window class** (`rembrowser-app-<site>`), so window rules and docks treat them as apps.

---

<div align="center">

Built with 🦀 Rust + [CEF](https://bitbucket.org/chromiumembedded/cef) · ad blocking by [adblock-rust](https://github.com/brave/adblock-rust)

</div>
