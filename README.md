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
| 🪟 **Windows** | run `rembrowser` again (or `:winopen`) for another window |
| 🧈 **Smooth scrolling** | `j` / `k` glide like a mouse wheel, also while the key is held |
| 🗂️ **Managers** | edit / delete bookmarks, history, pins, downloads and site permissions in the pickers |
| 🪶 **Light or full** | ad blocking, DRM and Chrome extensions are switches — you pick the weight |
| 🏠 **Homescreen** | clock, search box and your own site tiles (add / remove) |
| 📱 **Web apps** | any site as its own window: `rembrowser --app=https://web.whatsapp.com` |
| 📺 **Picture-in-picture** | `gp` floats the video |
| ✍️ **Edit fields in nvim** | `Ctrl-T` in any text box |
| 📜 **Userscripts** | Greasemonkey-style scripts — no extension needed |
| 🧩 **Chrome extensions** | optional (`extensions: true`), managed with `:extensions` |
| ⚙️ **Chromium settings** | `:settings` opens chrome://settings in its own window |
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
rembrowser update              # rembrowser, DRM, ad-block lists (system packages untouched)
rembrowser update --system     # the same after a full system upgrade (updates the Chromium engine)
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
| `;m` | play the link / video in mpv |
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
| 🖱️ mouse | click a tab to switch, `×` on a tab to close it |

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
| `<Space>sd` | ⬇️ downloads (kept across restarts) |
| `<Space>sq` | 📚 read-later list |
| `<Space>sk` | ⌨️ all shortcuts |
| `<Space>ma` / `<Space>mm` / `<Space>1-4` | 📌 harpoon: pin / list / jump |
| `<Space>.` / `<Space>S` | 📚 read later: add / open oldest |
| `<Space>ut` / `<Space>ul` | 📌 pin tab bar / status line |
| `<Space>ub` | 🔖 show / hide the bookmark bar |
| `<Space>uc` | ⚙️ edit config |
| `<Space>us` | ⚙️ Chromium settings |
| `<Space>ee` / `<Space>ep` | 🧩 extensions: manage / open a popup |
| `<Space>ei` / `<Space>ex` | 🧩 extensions: install from this Web Store page / remove one |
| `<Space>ec` | 🧩 this page in a Chromium window (toolbar, extension buttons) |

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

### 🗂️ Inside a picker (bookmarks, history, pins, downloads, …)
| Key | Action |
|---|---|
| `Enter` | open |
| `Ctrl-e` | edit (bookmarks, pins) |
| `Ctrl-x` | delete the entry |
| `Ctrl-o` / `Ctrl-d` | downloads: show folder / delete the file too |

🖱️ The same actions appear as buttons on the row under the mouse.

---

## 💬 Commands

| Command | |
|---|---|
| `:open aw niri` | search with a shortcut (`dd` `g` `gh` `aw` `yt` `w`) |
| `:tabs` `:history` `:bookmarks` `:downloads` `:keys` | pickers |
| `:queue` `:permissions` | read-later list · remembered site permissions |
| `:history-clear` `:downloads-clear` | forget all history / the download list |
| `:winopen [URL]` | new window |
| `:mpv [URL]` | play this page (or URL) in mpv |
| `:settings` `:extensions` `:chrome [URL]` | Chromium settings · extensions · a page in a Chromium window |
| `:ext-install [URL\|id]` `:ext-remove` | install an extension from the Web Store · remove one |
| `:app` / `:app-install` | turn this page into a web app / add it to your launcher |
| `:ext-popup` | open an extension's popup in its own window |
| `:pip` | picture-in-picture |
| `:set dark-mode!` | toggle any setting |
| `:config` | edit your config (`config.yaml`) |
| `:userscripts` | reload userscripts |

---

## ⚙️ Config

`rembrowser config` (or `:config`) opens `~/.config/rembrowser/config.yaml` — saving applies it instantly.

```yaml
settings:
  home-page: home          # "home" = the homescreen, or any URL
  status-bar: false        # true = always visible
  tab-bar: false
  bookmark-bar: false
  dark-mode: true
  smooth-scrolling: true
  confirm-quit: true       # ask before the last tab closes (d / x) and on Ctrl-q
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

### 🪶 Light or full
Everything that adds weight is a switch in `settings:` (read at startup — restart after changing one):

| Setting | Default | What it loads |
|---|---|---|
| `adblock` | `true` | filter lists in memory (~13 MB), list downloads |
| `drm` | `true` | Widevine, for Netflix / Spotify / … |
| `extensions` | `false` | Chrome extensions |
| `smooth-scrolling` | `true` | animated `j` / `k` (off = instant jumps) |
| `tab-bar` `bookmark-bar` `status-bar` | `false` | pinned bars |

🪶 Lightest: `adblock: false`, `drm: false`, `extensions: false`. 🧰 Fullest: all `true`.

### 🧩 Chrome extensions (optional)
1. Set `extensions: true` in the config.
2. Open the extension's [Chrome Web Store](https://chromewebstore.google.com) page and run `:ext-install` (or `:ext-install <store URL or id>` from anywhere). The store's own button only works in Chrome, so rembrowser downloads and unpacks the extension itself.
3. Restart. The extension now acts on your normal tabs (tested: Dark Reader, uBlock Origin Lite).

`:extensions` opens Chromium's extension page (options, on/off switches) · `:ext-popup` opens an extension's popup (the page behind its toolbar button) in its own window · `:ext-remove` deletes one · `:ext-install` again updates one.

Limits: tabs are not Chrome tabs, so an extension that lists tabs (`chrome.tabs.query`) sees none, and toolbar buttons only exist in `:chrome` windows (`:ext-popup` opens the popup without one). Extensions do not update on their own.

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

**🧩 Chrome extensions?** Optional — see *Chrome extensions* above. Ad blocking and userscripts are built in, so most setups need none.

**⚙️ `chrome://settings`?** `:settings` opens it in its own window (tabs run without Chromium's UI layer, which is what keeps them light). rembrowser's own options are in `:config` / `:set`.

**🪟 Second window?** Run `rembrowser` again, `rembrowser --new-window URL`, or `:winopen`. `rembrowser URL` adds a tab to the window you used last, and `rembrowser ':tabopen x.org'` runs any command in the running browser.

**🚪 Quit asks every time?** `confirm-quit: false`.

**🎬 DRM didn't play on first launch?** Widevine downloads in the background on first run — restart once.

**📱 Web apps keep their own window class** (`rembrowser-app-<site>`), so window rules and docks treat them as apps.

---

<div align="center">

Built with 🦀 Rust + [CEF](https://bitbucket.org/chromiumembedded/cef) · ad blocking by [adblock-rust](https://github.com/brave/adblock-rust)

</div>
