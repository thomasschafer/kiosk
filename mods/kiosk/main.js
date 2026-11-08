// Kiosk, the repository and branch picker, drawn natively.
//
// `mod.open:kiosk` opens this mod as a popup over the window (a dialog overlay). It runs `herdr-app-kiosk serve`, which starts
// the picker in the background and prints where to reach it: a loopback port and a secret.
// The mod connects, and from then on the picker (Rust, the same state machine as kiosk's
// terminal UI) sends what to show and the mod sends what the person does. The popup closes
// when the picker exits; closing the popup some other way tells the picker to quit.
//
// The binary is looked for in this folder's bin/, where installing puts it, then, when this
// folder is linked from a herdr-app-kiosk checkout, that checkout's target/release/, then
// on PATH. RECIPE.md says how to install it.

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { appHome, closeOverlay, exec, focused, nextCommand, notify, panes, props, readTextFile, setShared, settings, shared, spaces, theme, window, writeTextFile } from "herdr";
import { Buffer } from "buffer";
import net from "net";

// This folder, as the app rewrites it when the mod is cloned or renamed.
const MOD_DIR = "mods/kiosk";
const TITLE = "Kiosk";
// The release of herdr-app-kiosk this mod comes from, whose binary it runs.
const VERSION = "0.8.0";
const REPOSITORY = "github.com/thomasschafer/kiosk";
// The protocol this mod speaks; the picker refuses a different one.
const PROTOCOL = 4;

// The dialog's share of the window, as mod.json opens it. Kiosk draws its own panel inside
// it (mod.json's frame is none): hung from the top, and as tall as what it shows, so a short
// list makes a short panel. Each list shows as many rows as fit (see `viewportFor`), and the
// picker fills them.
const SHARE = 0.8;
// One set of measurements for the whole picker, so edges and text line up everywhere: a
// row's highlight sits PAD inside the panel and its icon INSET inside the highlight, and the
// search bar's icon and text line up with the icons and names below them. A dialog takes the
// panel's place, at the same width, rather than piling a second box on it.
const PANEL = 720;
const RADIUS = 12;
const BORDER = 1;
const PAD = 8;
const INSET = 12;
const ICON = 16;
const ICON_GAP = 12;
const ROW = 34;
// More rows than this make a wall rather than a list; the search narrows them instead.
const MAX_ROWS = 10;
const EMPTY = 116;
const HELP_ROW = 28;
// The line over help saying a notification is in front of its keys.
const NOTICE = 26;
const WARNING_ROW = 48;
const BAR = 52;
const TITLE_BAR = 48;
// A title bar with a line under its title.
const TALL_TITLE_BAR = 64;
const FOOTER = 40;
// A footer with buttons in it, which are taller than its hints.
const BUTTON_FOOTER = 52;
const TOAST = 36;
// The tab bar along the panel's top, and the space round its tabs.
const TAB_BAR = 46;
const TAB_INSET = 3;
const TAB = 30;
const EDGE = 20;
const ICONS = "mods/workspace-chrome";
const CONTEXT = "KioskPicker";
// The most a paste sends, well inside the app's 1 MiB limit on one socket write.
const MAX_PASTE = 128 * 1024;
// The longest snapshot of the spaces the mod passes the picker, well inside the 32,767
// characters Windows allows a whole command line. The picker asks the app itself without one.
const MAX_SNAPSHOT = 8 * 1024;
// Shared state between kiosk and the launcher mods (such as kiosk-diff) that open it on a
// tab. A launcher sets LAUNCH to the tab and opens kiosk, which takes it and clears it; while
// kiosk is open, OPEN names its overlay, and a launcher instead sends it `kiosk.tab:<tab>:<nonce>`,
// which kiosk acknowledges by setting ACK to the nonce. A launcher that hears nothing takes OPEN
// as left over from a crash. `kiosk.focus` gives kiosk the keyboard back once a launcher has gone.
const LAUNCH = "kiosk.launch";
const OPEN = "kiosk.open";
const ACK = "kiosk.ack";
// Where the picker saves notices it couldn't show because its popup had closed, one JSON
// object per line, below the app's home.
const SAVED_NOTICES = "data/kiosk/notices.jsonl";

// A failure to tell the person about: a notification's title, and what to do about it.
class KioskError extends Error {
  constructor(title, body) {
    super(`${title}: ${body}`);
    this.title = title;
    this.body = body;
  }
}

// Where the binary may be: the install script's copy, then a linked checkout's build.
function binaries() {
  const dir = `${appHome()}/${MOD_DIR}`;
  const exe = window().platform === "windows" ? ".exe" : "";
  return [`${dir}/bin/herdr-app-kiosk${exe}`, `${dir}/../../target/release/herdr-app-kiosk${exe}`, "herdr-app-kiosk"];
}

// Where a space is now: its first terminal's live folder (as the app names it).
function place(space) {
  if (!space) return null;
  const tabs = new Set(space.tabs.map((t) => t.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd ?? null;
}

function context() {
  const now = focused();
  const space = spaces().find((s) => s.id === now?.space && !s.machine);
  return { focused_pane_cwd: space ? now.cwd : null, workspace_cwd: place(space) };
}

// Each space on this machine with the live folder of each of its terminals, in the app's
// order, as JSON. The picker uses these instead of asking the app (a process for each
// terminal) until it changes the window itself. Null when a terminal's folder isn't known
// yet, which the picker would otherwise take as a space with no folder, or when there are too
// many to pass: either way the picker asks the app.
function snapshot() {
  const all = panes();
  const local = spaces()
    .filter((s) => !s.machine)
    .map((s) => ({
      id: s.id,
      folders: s.tabs.flatMap((t) => all.filter((p) => p.tab === t.id && p.terminal).map((p) => p.cwd ?? null)),
    }));
  if (local.some((s) => s.folders.includes(null))) return null;
  const json = JSON.stringify(local);
  return json.length <= MAX_SNAPSHOT ? json : null;
}

// Shows the notices the picker saved last time, such as an `[on_open]` command it couldn't type.
// The file is emptied only if nothing was added since it was read; otherwise its notices wait
// for the next time, rather than any being lost.
async function showSavedNotices() {
  const path = `${appHome()}/${SAVED_NOTICES}`;
  const text = await readTextFile(path);
  if (!text) return;
  try {
    await writeTextFile(path, "", text);
  } catch {
    return;
  }
  for (const line of text.split("\n").filter((l) => l.trim())) {
    let notice;
    try {
      notice = JSON.parse(line);
    } catch (error) {
      notice = { title: "Kiosk saved an unreadable notice", body: `${line} (${error})` };
    }
    notify({ title: notice.title, body: notice.body, level: "warning" });
  }
}

// Runs `herdr-app-kiosk serve` from the first place it can, opening on `tab` (the picker's
// first when null), returning where the picker listens.
async function launch(tab) {
  const spacesSnapshot = snapshot();
  const args = [
    "serve",
    "--app-home",
    appHome(),
    "--context",
    JSON.stringify(context()),
    ...(spacesSnapshot === null ? [] : ["--spaces", spacesSnapshot]),
    ...(tab === null ? [] : ["--tab", tab]),
  ];
  const missing = [];
  for (const program of binaries()) {
    let r;
    try {
      r = await exec(program, args);
    } catch (error) {
      missing.push(`${program} (${error})`);
      continue;
    }
    if (r.code !== 0) throw new KioskError("Kiosk didn't start", (r.stderr || r.stdout).trim() || `${program} exited with ${r.code}`);
    try {
      return { program, handshake: JSON.parse(r.stdout.trim()) };
    } catch {
      throw new KioskError("Kiosk didn't start", `${program} printed no handshake: ${r.stdout.trim()}`);
    }
  }
  throw new KioskError("Kiosk isn't installed", `Its program herdr-app-kiosk ${VERSION} isn't in ${MOD_DIR}/bin or on PATH; ${REPOSITORY} says how to install it. Tried ${missing.join("; ")}.`);
}

async function start(tab) {
  const { program, handshake } = await launch(tab);
  if (handshake.protocol !== PROTOCOL) {
    throw new KioskError("Kiosk needs updating", `This mod (herdr-app-kiosk ${VERSION}) speaks protocol ${PROTOCOL} and ${program} speaks ${handshake.protocol}. Install the binary of release v${VERSION} from ${REPOSITORY}.`);
  }
  const socket = await net.connect("127.0.0.1", handshake.port);
  return { socket, token: handshake.token };
}

// Lines of a socket's bytes, decoded once whole so a character split across reads survives.
class Lines {
  constructor() {
    this.pending = new Uint8Array(0);
  }

  push(chunk) {
    const joined = new Uint8Array(this.pending.length + chunk.length);
    joined.set(this.pending);
    joined.set(chunk, this.pending.length);
    const lines = [];
    let start = 0;
    for (let i = 0; i < joined.length; i++) {
      if (joined[i] === 10) {
        lines.push(Buffer.from(joined.subarray(start, i)).toString("utf8"));
        start = i + 1;
      }
    }
    this.pending = joined.slice(start);
    return lines;
  }
}

// How many rows each list shows in a dialog `height` px tall: what the panel's search bar,
// title and footer leave, up to a list's most.
// `tabbed` when the panel has a tab bar along its top.
function viewportFor(height, toast, tabbed) {
  const fit = (space, row, least, most) => Math.max(least, Math.min(most, Math.floor(space / row)));
  const panel = height - 2 * BORDER;
  const list = panel - (tabbed ? TAB_BAR : 0) - BAR - BORDER - 2 * PAD - BORDER - FOOTER - (toast ? TOAST + PAD : 0);
  return {
    list: fit(list, ROW, 1, MAX_ROWS),
    bases: fit(panel - TALL_TITLE_BAR - BORDER - BAR - BORDER - 2 * PAD - BORDER - BUTTON_FOOTER, ROW, 3, 8),
    help: fit(panel - TITLE_BAR - BORDER - BAR - BORDER - (toast ? NOTICE : 0) - 2 * PAD, HELP_ROW, 4, 18),
    scan_warnings: fit(panel - TITLE_BAR - BORDER - 2 * PAD - BORDER - FOOTER, WARNING_ROW, 2, 8),
  };
}

// `color` (#rrggbb, or #rrggbbaa whose alpha this replaces) at `opacity`.
function tint(color, opacity) {
  if (typeof color !== "string" || !/^#[0-9a-f]{6}([0-9a-f]{2})?$/i.test(color)) {
    throw new Error(`the theme's color ${JSON.stringify(color)} isn't #rrggbb`);
  }
  if (!(opacity >= 0 && opacity <= 1)) throw new Error(`an opacity of ${opacity}`);
  return color.slice(0, 7) + Math.round(opacity * 255).toString(16).padStart(2, "0");
}

// What each of the picker's tones looks like in the app's theme.
function toneColor(t, tone) {
  switch (tone) {
    case "muted":
    case "neutral":
      return t.muted;
    case "faint":
    case "hint":
      return t.dim;
    case "accent":
    case "secondary":
    case "tertiary":
      return t.accent;
    case "warning":
    case "attention":
      return t.blocked;
    case "error":
    case "failure":
      return t.ansi1;
    case "open":
    case "success":
      return t.done;
    case "progress":
      return t.working;
    default:
      return t.foreground;
  }
}

const icon = (t, name, color, size = 14) => svg(`${ICONS}/${name}.svg`).size(size).flex_none().text_color(color ?? t.dim);

function spinner(t, color, size = 12) {
  return svg(`${ICONS}/spinner.svg`).size(size).flex_none().text_color(color ?? t.dim).spin(900);
}

// A key as the app writes keys: symbols on macOS (⌃H), words elsewhere (Ctrl+H).
function keyLabel(key) {
  const mac = window().platform === "macos";
  const names = mac
    ? { "ctrl+": "⌃", "alt+": "⌥", "shift+": "⇧", enter: "↩", tab: "⇥", backspace: "⌫", delete: "⌦", esc: "esc" }
    : { "ctrl+": "Ctrl+", "alt+": "Alt+", "shift+": "Shift+", enter: "Enter", tab: "Tab", backspace: "Backspace", delete: "Delete", esc: "Esc" };
  return key.split("/").map((part) => {
    let rest = part;
    let out = "";
    for (const modifier of ["ctrl+", "alt+", "shift+"]) {
      if (rest.startsWith(modifier)) {
        out += names[modifier];
        rest = rest.slice(modifier.length);
      }
    }
    const arrows = { up: "↑", down: "↓", left: "←", right: "→" };
    // Other named keys are capitalised as keyboards print them: F1, Home, PageUp.
    const capitalised = { pageup: "PageUp", pagedown: "PageDown" }[rest] ?? rest.charAt(0).toUpperCase() + rest.slice(1);
    const base = names[rest] ?? arrows[rest.toLowerCase()] ?? (rest.length === 1 ? rest.toUpperCase() : capitalised);
    return out + base;
  }).join("/");
}

function keycap(t, label) {
  return div().flex_none().min_w(18).px(5).h(18).flex().items_center().justify_center().rounded(4)
    .bg(tint(t.foreground, 0.06)).border_1().border_color(tint(t.foreground, settings().dark ? 0.08 : 0.12))
    .text_size(11).font_medium().text_color(t.muted).child(keyLabel(label));
}

// The picker's key hints. Leaving is always the last thing a person needs, so its `quit`
// reads as closing.
function hints(t, list) {
  return h_flex().flex_wrap().gap_x(18).gap_y(4).items_center().text_size(12).text_color(t.muted)
    .children(list.map((h) => h_flex().gap(5).items_center().child(keycap(t, h.key)).child(h.label === "quit" ? "close" : h.label)));
}

// Help and leaving go at the footer's end, apart from what can be done to the selection.
const TRAILING_HINTS = new Set(["help", "quit"]);

// How each badge reads in a row. The main checkout goes without saying.
const BADGES = {
  main_checkout: null,
  folder: (t) => ["folder", t.muted],
  worktree: (t) => ["worktree", t.accent],
  current: (t) => ["current", t.done],
  default: (t) => ["default", t.muted],
  remote: (t, b) => [b.name, t.muted],
};

function shownBadges(badges) {
  return badges.filter((b) => BADGES[b.kind] !== null);
}

function badge(t, b) {
  const [label, color] = (BADGES[b.kind] ?? ((tt) => [b.kind, tt.muted]))(t, b);
  return div().flex_none().h(18).px(6).flex().items_center().rounded(4).bg(tint(color, 0.13))
    .text_size(11).font_medium().text_color(color).child(label);
}

// What the picker has typed, with the cursor drawn where it is while typing goes there
// (`active`); the placeholder when nothing is.
function queryText(t, query, placeholder, active, size) {
  const empty = !query.before && !query.after;
  // The cursor takes no room, so moving it through the text leaves the characters where they are.
  const cursor = active
    ? div().w(0).h(size + 3).flex_none().relative()
      .child(div().absolute().top(0).left(-0.75).w(1.5).h(size + 3).rounded(1).bg(t.accent).loop("opacity", { from: 0.15, to: 1, period: 1100 }))
    : div();
  return h_flex().flex_1().min_w(0).overflow_hidden().items_center().whitespace_nowrap().text_size(size)
    .child(div().flex_none().text_color(t.foreground).child(query.before))
    .child(cursor)
    .when(empty, (el) => el.child(div().pl(1).truncate().text_color(t.dim).child(placeholder)))
    .when(!empty, (el) => el.child(div().flex_none().text_color(t.foreground).child(query.after)));
}

// A search field in the setup wizard.
function field(t, query, placeholder, active = true) {
  return h_flex().h(38).flex_none().px(INSET).gap(ICON_GAP).items_center().rounded(8).border_1()
    .border_color(active ? tint(t.accent, 0.7) : t.border).bg(t.background)
    .child(icon(t, "search", t.dim, ICON))
    .child(queryText(t, query, placeholder, active, 13.5));
}

// The panel everything sits in. It keeps its clicks, so a click on it doesn't count as one
// beside it.
function panel(t, id = "kiosk-panel") {
  return v_flex().id(id).on_click((_e, cx) => cx.stop_propagation()).w(PANEL).max_w_full().flex_none().relative()
    .rounded(RADIUS).bg(t.popover).border_1().border_color(t.border).shadow_2xl().text_size(13);
}

// The selected row's fill: a light theme needs more of the accent to show it.
function selectedFill(t) {
  return tint(t.accent, settings().dark ? 0.13 : 0.16);
}

// `text` with the characters at `matches` (character indices, ascending) marked in `color`:
// where the search matched it.
function marked(text, matches, color) {
  const characters = Array.from(text);
  matches.forEach((position, i) => {
    if (!Number.isInteger(position) || position < 0 || position >= characters.length || (i > 0 && position <= matches[i - 1])) {
      throw new Error(`the picker marked ${JSON.stringify(matches)} in ${JSON.stringify(text)}`);
    }
  });
  const marks = new Set(matches);
  const runs = [];
  characters.forEach((character, i) => {
    const mark = marks.has(i);
    if (runs.length && runs[runs.length - 1].mark === mark) runs[runs.length - 1].text += character;
    else runs.push({ mark, text: character });
  });
  return runs.map((run) => run.mark ? div().flex_none().text_color(color).font_semibold().child(run.text) : div().flex_none().child(run.text));
}

function divider(t) {
  return div().h(BORDER).flex_none().bg(t.border);
}

function heading(t, text, size = 15) {
  return div().text_size(size).font_semibold().text_color(t.foreground).child(text);
}

/**
 * Where the popup's connection to the picker is. It is starting until the picker has been
 * launched and connected to, then connected, writing to `socket` one line after another
 * through `writes`, and closed once the popup is closing, after which nothing is sent.
 * @typedef {{ state: "starting" }
 *   | { state: "connected", socket: any, writes: Promise<void> }
 *   | { state: "closed" }} Link
 */

export default class Kiosk extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.timer.after(600, () => this.handle.focus());
    this.view = null;
    this.chords = [];
    // Whether the picker has said hello.
    this.ready = false;
    // The rows last told to the picker, once it has said hello; render keeps them current.
    this.viewport = null;
    /** @type {Link} */
    this.link = { state: "starting" };
    this.wheel = 0;
    // A launcher's tab, sent once the picker has said hello, for one that came while it started.
    this.pendingLaunch = null;
    // `herdr-app check` loads the mod with no overlay; only a shown copy starts the picker.
    if (!props()?.overlay) return;
    const tab = shared(LAUNCH) ?? null;
    if (tab !== null) setShared(LAUNCH, null);
    setShared(OPEN, { overlay: props().overlay });
    cx.spawn(async (cx) => {
      for (;;) {
        this.command(await nextCommand());
        cx.notify();
      }
    });
    cx.spawn(async () => {
      try {
        await showSavedNotices();
      } catch (error) {
        notify({ title: "Kiosk couldn't show its saved notices", body: String(error), level: "warning" });
      }
    });
    cx.spawn(async (cx) => {
      try {
        const { socket, token } = await start(tab);
        if (this.link.state === "closed") {
          socket.close();
          return;
        }
        this.link = { state: "connected", socket, writes: Promise.resolve() };
        this.send({ t: "hello", token, protocol: PROTOCOL });
        await this.listen(socket, cx);
      } catch (error) {
        if (this.link.state === "closed") return;
        const known = error instanceof KioskError;
        notify({ title: known ? error.title : "Kiosk stopped", body: known ? error.body : String(error), level: "error" });
        this.finish();
      }
    });
  }

  async listen(socket, cx) {
    const lines = new Lines();
    for (;;) {
      let chunk;
      try {
        chunk = await socket.read();
      } catch (error) {
        // A read gives up after 30 s; the picker pings every 10, so a timeout means it's gone.
        throw new KioskError("Kiosk stopped responding", String(error));
      }
      if (chunk === null) {
        if (this.link.state !== "closed") throw new KioskError("Kiosk stopped", "The picker closed its connection.");
        return;
      }
      for (const line of lines.push(chunk)) {
        if (this.receive(JSON.parse(line), cx)) return;
      }
      cx.notify();
    }
  }

  // Applies one message from the picker. Returns whether the session is over.
  receive(message, cx) {
    switch (message.t) {
      case "hello":
        this.chords = message.chords;
        cx.bind_keys(this.chords.map((keystroke) => ({ keystroke, action: `kiosk.key:${keystroke}`, context: CONTEXT })));
        this.ready = true;
        if (this.pendingLaunch !== null) this.send({ t: "launch", tab: this.pendingLaunch });
        this.pendingLaunch = null;
        return false;
      case "view":
        this.view = message.view;
        return false;
      case "notify":
        notify({ title: message.title, body: message.body, level: message.level });
        return false;
      case "error":
        notify({ title: message.title, body: message.body, level: "error" });
        this.finish();
        return true;
      case "exit":
        this.finish();
        return true;
      case "ping":
        return false;
      default:
        throw new KioskError("Kiosk stopped", `The picker sent a message this mod doesn't know: ${JSON.stringify(message)}`);
    }
  }

  send(message) {
    const link = this.link;
    if (link.state !== "connected") return;
    const line = `${JSON.stringify(message)}\n`;
    // One write at a time, so the picker sees the person's input in order. A write that
    // fails loses input, so the picker can no longer be trusted to show what was done.
    link.writes = link.writes.then(() => link.socket.write(line)).catch((error) => {
      if (this.link.state === "closed") return;
      notify({ title: "Kiosk stopped responding", body: `Sending to the picker failed: ${error}`, level: "error" });
      this.finish();
    });
  }

  // A command a launcher mod sent: `tab:<tab>[:<nonce>]` asks for a tab, and `focus` gives
  // kiosk the keyboard back.
  command(command) {
    if (command === "focus") {
      this.handle.focus();
      return;
    }
    const [name, tab, nonce] = command.split(":");
    if (name !== "tab" || !tab) {
      notify({ title: "Kiosk got a command it doesn't know", body: command, level: "warning" });
      return;
    }
    if (nonce) setShared(ACK, nonce);
    if (this.ready) this.send({ t: "launch", tab });
    else this.pendingLaunch = tab;
  }

  // Says kiosk is no longer open, unless another copy has said it is since.
  forget() {
    if (shared(OPEN)?.overlay === props()?.overlay) setShared(OPEN, null);
  }

  finish() {
    const link = this.link;
    if (link.state === "closed") return;
    this.link = { state: "closed" };
    if (link.state === "connected") link.socket.close();
    this.forget();
    closeOverlay(props().overlay);
  }

  // A popup closed while still starting has no picker to tell, as its startup ends with
  // it; a picker no one connects to gives up on its own.
  async close() {
    this.forget();
    const link = this.link;
    if (link.state === "closed") return;
    this.link = { state: "closed" };
    if (link.state !== "connected") return;
    try {
      await link.writes;
      await link.socket.write(`${JSON.stringify({ t: "close" })}\n`);
    } catch {}
    link.socket.close();
  }

  key(e, cx) {
    const mac = window().platform === "macos";
    const pasting = e.key === "v" && (mac ? e.modifiers.platform && !e.modifiers.control : e.modifiers.control && !e.modifiers.platform);
    if (pasting || (e.key === "insert" && e.modifiers.shift)) {
      const text = cx.read_from_clipboard();
      if (text && Buffer.from(text).length > MAX_PASTE) {
        notify({ title: "Kiosk didn't paste", body: "That is too much text to search for.", level: "warning" });
      } else if (text) {
        this.send({ t: "paste", text });
      }
      cx.stop_propagation();
      return;
    }
    // The platform key is the app's; the picker binds none.
    if (e.modifiers.platform) return;
    this.send({ t: "key", keystroke: e.keystroke, key_char: e.key_char ?? null });
    cx.stop_propagation();
  }

  scroll(e, cx) {
    this.wheel -= (e.delta_lines?.y ?? e.delta.y / ROW);
    const rows = Math.trunc(this.wheel);
    if (rows !== 0) {
      this.wheel -= rows;
      this.send({ t: "scroll", delta: rows });
    }
    cx.stop_propagation();
  }

  // Tells the picker how many rows fit, when that changes: the window was resized, or a
  // notification took room.
  fitViewport() {
    if (!this.ready) return;
    // The app draws a mod's sizes at its zoom; the dialog's share of the window is not.
    const zoom = settings().zoom;
    if (!(zoom > 0)) throw new Error(`the app's zoom is ${zoom}`);
    const height = (globalThis.window.viewport_size().height * SHARE) / zoom;
    const view = this.view?.screen === "tabs" ? this.view.body : this.view;
    const viewport = viewportFor(height, !!view?.toast, this.view?.bar?.length > 0);
    if (JSON.stringify(viewport) === JSON.stringify(this.viewport)) return;
    this.viewport = viewport;
    this.send({ t: "viewport", ...viewport });
  }

  // The dialog's whole area is this mod's, though the panel only fills its top: a click on
  // the rest is a click beside the picker, which closes it as one beside the dialog would.
  // That click is caught below the element holding the keyboard, as GPUI also clicks a
  // focused element when space is pressed, and space is typed into the search.
  render() {
    this.fitViewport();
    const t = theme();
    let root = v_flex().id("kiosk").key_context(CONTEXT).track_focus(this.handle).size_full().min_h(0)
      .text_size(13).text_color(t.foreground)
      .on_key_down((e, cx) => this.key(e, cx));
    for (const keystroke of this.chords) {
      root = root.on_action(`kiosk.key:${keystroke}`, () => this.send({ t: "key", keystroke, key_char: null }));
    }
    const beside = v_flex().id("kiosk-beside").size_full().min_h(0).relative().items_center()
      .on_click((_e, cx) => { cx.stop_propagation(); this.finish(); });
    return root.child(beside.child(this.screen(t)));
  }

  screen(t) {
    const v = this.view;
    if (!v) return this.message(t, spinner(t), "Starting…");
    if (v.screen === "setup") return this.setup(t, v);
    if (v.screen === "no_search_dirs") return this.noSearchDirs(t, v);
    if (v.screen === "tabs") return this.tabs(t, v);
    throw new Error(`the picker sent a screen this mod doesn't know: ${v.screen}`);
  }

  // The tab that shows, with the tab bar along its panel's top.
  tabs(t, v) {
    const body = v.body;
    switch (body.tab) {
      case "picker":
        return this.picker(t, body, v.bar);
      case "diff":
        return this.diff(t, body, v.bar);
      default:
        throw new Error(`the picker sent a tab this mod doesn't know: ${body.tab}`);
    }
  }

  // Each tab with the key that shows it, centred so the tabs stay put as the panel changes
  // width between tabs. Nothing while kiosk shows one tab.
  tabBar(t, bar) {
    if (!bar.length) return null;
    return h_flex().id("kiosk-tabs").h(TAB_BAR).flex_none().justify_center().items_center().border_b_1().border_color(t.border)
      .child(h_flex().p(TAB_INSET).gap(2).items_center().rounded(9).bg(tint(t.foreground, 0.05))
        .children(bar.map((tab) =>
          h_flex().id(`kiosk-tab-${tab.id}`).h(TAB).px(10).gap(8).items_center().rounded(6).cursor_pointer().text_size(12.5).font_medium()
            .when(tab.active, (el) => el.bg(t.popover).text_color(t.foreground).shadow_sm())
            .when(!tab.active, (el) => el.text_color(t.muted).hover((s) => s.text_color(t.foreground)))
            .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "show_tab", tab: tab.id }); })
            .child(tab.title)
            .when(!!tab.key, (el) => el.child(keycap(t, tab.key))))));
  }

  // The diff tab: the whole dialog, as a diff needs the room.
  diff(t, d, bar) {
    const tabs = this.tabBar(t, bar);
    return v_flex().id("kiosk-diff").size_full().min_h(0)
      .on_click((_e, cx) => cx.stop_propagation())
      .rounded(RADIUS).bg(t.popover).border_1().border_color(t.border).shadow_2xl()
      .child(tabs ?? this.titleBar(t, "Diff"))
      .child(v_flex().flex_1().items_center().justify_center().gap(6)
        .child(div().text_size(14).text_color(t.foreground).child(d.message))
        .when(!!d.detail, (el) => el.child(div().text_size(12.5).text_color(t.muted).child(d.detail))))
      .child(this.footer(t, d.hints));
  }

  // A panel with one centred line of news, such as the picker starting.
  message(t, lead, text, hint) {
    return panel(t)
      .child(this.titleBar(t, TITLE))
      .child(v_flex().h(140).items_center().justify_center().gap(8)
        .child(h_flex().gap(10).items_center().child(lead).child(div().text_color(t.muted).child(text)))
        .when(!!hint, (el) => el.child(div().text_size(12).text_color(t.dim).child(hint))));
  }

  // A panel's title over a divider: `after` follows the title, `subtitle` goes under it and
  // `right` at the end.
  titleBar(t, title, { after, subtitle, right } = {}) {
    return h_flex().h(subtitle ? TALL_TITLE_BAR : TITLE_BAR).flex_none().px(EDGE).gap(10).items_center().border_b_1().border_color(t.border)
      .child(v_flex().flex_1().min_w(0).gap(3)
        .child(h_flex().min_w(0).gap(8).items_center()
          .child(div().min_w(0).truncate().text_size(14).font_semibold().child(title))
          .when(!!after, (el) => el.child(after)))
        .when(!!subtitle, (el) => el.child(div().truncate().text_size(12.5).text_color(t.muted).child(subtitle))))
      .when(!!right, (el) => el.child(right));
  }

  // A button for one of the picker's commands; `kind` is `primary` for the one Enter does, or
  // `danger` when that one can't be undone.
  button(t, id, label, command, kind = null) {
    return h_flex().id(`kiosk-button-${id}`).flex_none().h(30).px(14).rounded(7).items_center().text_size(12.5).font_medium().cursor_pointer()
      .when(kind === "primary", (el) => el.bg(t.accent).text_color(t.accent_foreground).hover((s) => s.opacity(0.9)))
      .when(kind === "danger", (el) => el.bg(t.ansi1).text_color(t.popover).hover((s) => s.opacity(0.9)))
      .when(kind === null, (el) => el.bg(tint(t.foreground, 0.07)).text_color(t.foreground).hover((s) => s.bg(tint(t.foreground, 0.11))))
      .when(command !== null, (el) => el.on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "command", command }); }))
      .child(label);
  }

  dismissButton(t) {
    return this.button(t, "cancel", "Cancel", null)
      .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "dismiss" }); });
  }

  // A setup wizard button: the wizard is driven by keys, so it presses `keystroke`.
  keyButton(t, id, label, keystroke, kind = null) {
    return this.button(t, id, label, null, kind)
      .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "key", keystroke, key_char: null }); });
  }

  // The panel's last line: the picker's hints, with help and leaving at the end unless
  // `buttons` take it.
  footer(t, list, ...buttons) {
    const leading = buttons.length ? list : list.filter((h) => !TRAILING_HINTS.has(h.label));
    const trailing = buttons.length ? [] : list.filter((h) => TRAILING_HINTS.has(h.label));
    return h_flex().flex_none().h(buttons.length ? BUTTON_FOOTER : FOOTER).px(PAD + INSET).gap(16).items_center()
      .border_t_1().border_color(t.border).rounded_b(RADIUS).bg(tint(t.foreground, 0.025))
      .child(div().flex_1().min_w(0).child(hints(t, leading)))
      .when(trailing.length > 0, (el) => el.child(hints(t, trailing)))
      .when(buttons.length > 0, (el) => el.child(h_flex().flex_none().gap(8).children(buttons)));
  }

  // A search bar: the query, what it searches within (`scope`) before it, and `right` at its end.
  searchBar(t, query, placeholder, active, scope, right) {
    return h_flex().h(BAR).flex_none().pl(PAD + INSET).pr(EDGE).gap(ICON_GAP).items_center()
      .child(icon(t, "search", active ? t.muted : t.dim, ICON))
      .child(h_flex().flex_1().min_w(0).gap(8).items_center()
        .when(!!scope, (el) => el.child(scope))
        .child(queryText(t, query, placeholder, active, 15)))
      .when(!!right, (el) => el.child(right));
  }

  // In a repository's branches, the repository goes before the search, and clicking it goes
  // back to the repositories.
  scope(t, list) {
    if (list.kind !== "branches") return null;
    return h_flex().id("kiosk-back").flex_none().h(26).pl(5).pr(9).gap(4).items_center().rounded(6).cursor_pointer()
      .bg(tint(t.foreground, 0.07)).hover((s) => s.bg(tint(t.foreground, 0.12)))
      .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "command", command: "repositories" }); })
      .child(icon(t, "chevron-left", t.dim, 13))
      .child(div().text_size(13).font_medium().text_color(t.foreground).child(list.subject ?? "Repository"));
  }

  // The picker, or the dialog over it in its place. `bar` is the tab bar's tabs, which go along
  // the picker's top but not a dialog's.
  picker(t, v, bar) {
    const main = v.main;
    const dismiss = () => this.send({ t: "dismiss" });
    if (v.dialog) return this.layer(this.dialog(t, v.dialog, v.hints), dismiss);
    if (v.overlay?.kind === "help") return this.layer(this.help(t, v.overlay), dismiss);
    if (v.overlay?.kind === "scan_warnings") return this.layer(this.scanWarnings(t, v.overlay), dismiss);
    if (main.state === "loading") return this.message(t, spinner(t, t.accent, 14), main.message, main.hint);
    return panel(t)
      .when(bar.length > 0, (el) => el.child(this.tabBar(t, bar)))
      .child(this.searchBar(t, main.query, main.placeholder, true, this.scope(t, main), this.summary(t, main)))
      .child(divider(t))
      .child(div().p(PAD).child(this.list(t, main, "list", (row, selected) => this.row(t, row, selected))))
      .when(!!v.toast, (el) => el.child(div().px(PAD).pb(PAD).child(this.toast(t, v.toast))))
      .child(this.footer(t, v.hints));
  }

  // What holds a dialog: a click beside it runs `dismiss` rather than closing the picker.
  layer(child, dismiss) {
    return v_flex().id("kiosk-layer").size_full().items_center()
      .on_click((_e, cx) => { cx.stop_propagation(); dismiss(); })
      .child(child);
  }

  summary(t, list) {
    return h_flex().flex_none().gap(6).items_center().text_size(12).text_color(t.dim)
      .when(!!list.activity, (el) => el.child(spinner(t)).child(`${list.activity}…`).child(div().text_color(t.border).child("·")))
      .child(list.summary);
  }

  // A list of the picker's rows, each one ROW tall, and as many as it was sent: a short list
  // makes a short panel. With more than fit, a thumb at its side says where the view is.
  list(t, list, name, row) {
    const area = v_flex().id(`kiosk-${name}`).flex_none().relative().overflow_hidden()
      .on_scroll_wheel((e, cx) => this.scroll(e, cx));
    if (!list.rows.length) {
      // The picker says what happened, then what can be done about it, as two sentences.
      const empty = list.empty ?? "";
      const split = empty.indexOf(". ");
      const [what, next] = split < 0 ? [empty, null] : [empty.slice(0, split), empty.slice(split + 2)];
      return area.h(EMPTY).items_center().justify_center().gap(4)
        .child(div().size(32).mb(8).flex().items_center().justify_center().rounded(8).bg(tint(t.foreground, 0.06))
          .child(icon(t, list.kind === "repos" ? "search" : "git-branch", t.muted, 16)))
        .child(div().text_size(13.5).text_color(t.foreground).child(what))
        .when(!!next, (el) => el.child(div().text_size(12.5).text_color(t.muted).child(next.charAt(0).toUpperCase() + next.slice(1))));
    }
    const height = list.rows.length * ROW;
    const scrolled = list.total > list.rows.length;
    return area.h(height).when(scrolled, (el) => el.pr(8))
      .children(list.rows.map((r, i) => row(r, i === list.selected)))
      .when(scrolled, (el) => el.child(this.thumb(t, list.offset, list.rows.length, list.total, height, 0)));
  }

  // Where a scrolled list's view is: `shown` of `total` from `offset`, in a track `height`
  // tall that starts `top` down.
  thumb(t, offset, shown, total, height, top) {
    return div().absolute().right(0).w(3).rounded(2).bg(tint(t.foreground, 0.16))
      .top(top + (offset / total) * height).h(Math.max(16, (shown / total) * height));
  }

  row(t, r, selected) {
    const id = r.id;
    const hovered = this.hovered === id;
    const folder = r.badges.some((b) => b.kind === "folder");
    const glyph = r.actions.branches || folder ? "folder" : "git-branch";
    const remote = r.badges.find((b) => b.kind === "remote");
    const action = (name, iconName, label, command) =>
      h_flex().id(`kiosk-${name}-${id}`).flex_none().gap(5).px(8).h(22).rounded(5).items_center().text_size(12).text_color(t.muted).cursor_pointer()
        .bg(tint(t.foreground, 0.07)).hover((s) => s.bg(tint(t.foreground, 0.12)).text_color(t.foreground))
        .on_click((_e, cx) => { cx.stop_propagation(); this.send({ t: "activate", position: r.position, id: r.id, command }); })
        .child(icon(t, iconName, null, 12))
        .child(label);
    // A branch only a remote has reads as the remote's: `origin/` before its name.
    const name = h_flex().flex_none().max_w("60%").min_w(0).items_center().text_size(13.5)
      .when(!!remote, (el) => el.child(div().flex_none().text_color(t.dim).child(`${remote?.name}/`)))
      .child(h_flex().min_w(0).overflow_hidden().whitespace_nowrap().text_color(remote ? t.foreground : toneColor(t, r.name.tone)).when(selected, (el) => el.font_medium())
        .children(marked(r.name.text, r.matches, t.accent)));
    return h_flex().id(`kiosk-row-${id}`).h(ROW).flex_none().px(INSET).rounded(7).gap(ICON_GAP).items_center().cursor_pointer()
      .when(selected, (el) => el.bg(selectedFill(t)))
      .when(!selected, (el) => el.hover((s) => s.bg(t.hover)))
      .on_hover((on, cx) => {
        if (on) this.hovered = id;
        else if (this.hovered === id) this.hovered = null;
        cx.notify();
      })
      .on_click(() => this.send({ t: "activate", position: r.position, id: r.id }))
      .child(icon(t, glyph, selected ? t.accent : t.dim, ICON))
      .child(name)
      .children(shownBadges(r.badges).filter((b) => b.kind !== "remote").map((b) => badge(t, b)))
      .child(div().flex_1())
      .children(r.labels.map((l) => div().flex_none().text_size(12).text_color(toneColor(t, l.tone)).child(l.text)))
      .when(r.open && !hovered, (el) => el.child(h_flex().flex_none().gap(6).items_center().text_size(12).text_color(t.muted)
        .child(div().size(6).rounded(3).bg(t.done)).child("open")))
      .when(hovered && r.actions.branches, (el) => el.child(action("branches", "git-branch", "Branches", "branches")))
      .when(hovered && r.actions.delete, (el) => el.child(action("delete", "x", "Remove", "delete")));
  }

  toast(t, toast) {
    const color = toneColor(t, toast.level);
    return h_flex().flex_none().h(TOAST).gap(10).px(INSET).rounded(8).items_center().bg(tint(color, 0.08)).border_1().border_color(tint(color, 0.25))
      .child(div().flex_none().size(7).rounded(4).bg(color))
      .child(div().flex_1().min_w(0).truncate().text_size(12.5).text_color(t.foreground).child(toast.message))
      .when(!!toast.counter, (el) => el.child(div().flex_none().text_size(11.5).text_color(t.dim).child(toast.counter)))
      .child(hints(t, toast.hints));
  }

  // A dialog in the panel's place. `footer` is the picker's hints, which follow the dialog.
  dialog(t, d, footer) {
    const body = (...children) => v_flex().px(EDGE).py(18).gap(14).children(children);
    switch (d.kind) {
      case "validating_branch":
        return panel(t, "kiosk-dialog")
          .child(this.titleBar(t, d.title))
          .child(body(h_flex().gap(10).items_center().child(spinner(t, t.accent)).child(div().text_color(t.muted).child(d.message))));
      case "base_branch":
        return panel(t, "kiosk-dialog")
          .child(this.titleBar(t, "New branch", {
            after: d.list.subject
              ? h_flex().flex_none().h(22).px(8).gap(5).items_center().rounded(5).bg(selectedFill(t)).text_size(12.5).font_medium().text_color(t.accent)
                .child(icon(t, "git-branch", t.accent, 12)).child(d.list.subject)
              : null,
            subtitle: "Pick the branch to start it from",
          }))
          .child(this.searchBar(t, d.list.query, d.list.placeholder, true, null, this.summary(t, d.list)))
          .child(divider(t))
          .child(div().p(PAD).child(this.list(t, d.list, "bases", (row, selected) => this.row(t, row, selected))))
          .child(this.footer(t, footer, this.dismissButton(t), this.button(t, "create", "Create branch", "open", "primary")));
      case "remote_branch":
        return panel(t, "kiosk-dialog")
          .child(this.titleBar(t, d.title))
          .child(body(...d.lines.map((line) => div().text_color(t.muted).child(line))))
          .child(v_flex().px(PAD).pb(PAD).children(d.choices.map((choice, i) =>
            h_flex().id(`kiosk-choice-${i}`).px(INSET).h(ROW).gap(ICON_GAP).rounded(7).items_center().cursor_pointer()
              .when(i === d.selected, (el) => el.bg(selectedFill(t)))
              .when(i !== d.selected, (el) => el.hover((s) => s.bg(t.hover)))
              .on_click(() => this.send({ t: "activate", position: i, id: choice.id }))
              .child(icon(t, "git-branch", i === d.selected ? t.accent : t.dim, ICON))
              .child(choice.label))))
          .child(this.footer(t, d.hints, this.dismissButton(t), this.button(t, "continue", "Continue", "open", "primary")));
      case "confirm_delete":
        return panel(t, "kiosk-dialog")
          .child(this.titleBar(t, d.force ? "Force-remove this checkout?" : "Remove this checkout?"))
          .child(body(
            ...(d.warning ? [h_flex().gap(10).items_center().text_size(12.5).text_color(t.foreground)
              .child(div().flex_none().size(7).rounded(4).bg(t.blocked)).child(d.warning)] : []),
            h_flex().px(INSET).h(ROW).gap(ICON_GAP).items_center().rounded(7).bg(tint(t.foreground, 0.05))
              .child(icon(t, "folder", t.dim, ICON))
              .child(div().flex_1().min_w(0).truncate().text_size(12.5).text_color(t.foreground).child(d.path)),
            div().text_size(12.5).text_color(t.muted).child(d.closes_in
              ? `Its space in ${d.closes_in} closes too. The branch stays.`
              : "The branch stays."),
          ))
          .child(d.busy
            ? h_flex().h(BUTTON_FOOTER).px(EDGE).gap(8).items_center().border_t_1().border_color(t.border).child(spinner(t, t.accent)).child(div().text_color(t.muted).child(d.busy))
            : this.footer(t, footer, this.dismissButton(t), this.button(t, "remove", d.force ? "Force-remove" : "Remove checkout", "open", "danger")));
      default:
        throw new Error(`the picker sent a dialog this mod doesn't know: ${d.kind}`);
    }
  }

  help(t, h) {
    // The picker sends as many entries as fit, so the list keeps its height as it scrolls.
    const height = h.empty ? EMPTY : Math.min(this.rows("help"), h.total) * HELP_ROW;
    // The picker names the key that closes help only in its summary.
    const closing = /^(.*) \((\S+): close\)$/.exec(h.summary);
    return panel(t, "kiosk-dialog")
      .child(this.titleBar(t, "Keyboard shortcuts"))
      .child(this.searchBar(t, h.query, h.placeholder, true, null, h_flex().flex_none().gap(14).items_center()
        .child(div().text_size(12).text_color(t.dim).child(closing ? closing[1] : h.summary))
        .when(!!closing, (el) => el.child(hints(t, [{ key: closing[2], label: "quit" }])))))
      .child(divider(t))
      .when(!!h.notice, (el) => el.child(div().h(NOTICE).flex_none().px(EDGE).flex().items_end().text_size(12).text_color(t.muted).child(h.notice)))
      .child(
        v_flex().id("kiosk-help").p(PAD).h(height + 2 * PAD).flex_none().relative().overflow_hidden()
          .on_scroll_wheel((e, cx) => this.scroll(e, cx))
          .when(h.total > h.entries.length, (el) => el.child(this.thumb(t, h.offset, h.entries.length, h.total, height, PAD)))
          .when(!!h.empty, (el) => el.child(div().h(EMPTY).flex().items_center().justify_center().text_color(t.muted).child(h.empty)))
          .children(h.entries.map((entry, i) => {
            const line = h_flex().h(HELP_ROW).flex_none().items_center().px(INSET);
            if (entry.kind === "section") return line.items_end().pb(4).text_size(11).font_semibold().text_color(t.dim).child(entry.name.toUpperCase());
            if (entry.kind === "blank") return line.h(HELP_ROW / 2);
            const selected = i === h.selected;
            return line.id(`kiosk-help-${entry.position}`).gap(12).rounded(7).cursor_pointer()
              .when(selected, (el) => el.bg(selectedFill(t)))
              .when(!selected, (el) => el.hover((s) => s.bg(t.hover)))
              .on_click(() => this.send({ t: "activate", position: entry.position, id: entry.id }))
              .child(div().flex_1().min_w(0).truncate().text_size(13).child(entry.description))
              .child(h_flex().flex_none().gap(4).justify_end().children(entry.keys.map((k) => keycap(t, k))));
          })),
      );
  }

  scanWarnings(t, s) {
    return panel(t, "kiosk-dialog")
      .child(this.titleBar(t, s.title))
      .child(
        v_flex().id("kiosk-warnings").p(PAD).h(Math.min(this.rows("scan_warnings"), Math.max(s.warnings.length, 1)) * WARNING_ROW + 2 * PAD).flex_none().overflow_hidden()
          .on_scroll_wheel((e, cx) => this.scroll(e, cx))
          .children(s.warnings.map((w) =>
            h_flex().h(WARNING_ROW).flex_none().px(INSET).gap(ICON_GAP).items_center()
              // The dot lines up with the path, the line the warning is about.
              .child(div().flex_none().w(ICON).h(34).pt(6).flex().justify_center().child(div().size(7).rounded(4).bg(t.blocked)))
              .child(v_flex().flex_1().min_w(0).gap(2)
                .child(div().truncate().text_size(13).child(w.path))
                .child(div().truncate().text_size(12).text_color(t.muted).child(w.message))))),
      )
      .child(this.footer(t, s.hints));
  }

  // The rows `name` shows, as last told to the picker; views come only after that.
  rows(name) {
    if (!this.viewport) throw new Error(`a view came before the picker knew how many rows fit`);
    return this.viewport[name];
  }

  setup(t, v) {
    const page = (footer, ...children) =>
      panel(t)
        .child(this.titleBar(t, `Set up ${TITLE}`))
        .child(v_flex().px(EDGE).py(18).gap(16).children(children))
        .child(footer);
    const label = (text) => div().text_size(11).font_semibold().text_color(t.dim).child(text.toUpperCase());
    const path = (text) => div().h(22).px(7).flex().items_center().rounded(5).bg(tint(t.foreground, 0.06)).text_size(12).text_color(t.muted).truncate().child(text);
    const added = (dirs) =>
      v_flex().gap(4).children(dirs.map((dir) =>
        h_flex().h(40).px(INSET).gap(ICON_GAP).items_center().rounded(8).bg(tint(t.foreground, 0.045))
          .child(icon(t, "folder", t.muted, ICON))
          .child(div().flex_1().min_w(0).truncate().child(dir.path))
          .child(div().text_size(12).text_color(t.dim).child(dir.depth))
          .when(dir.all_folders, (el) => el.child(badge(t, { kind: "folder" })))));
    switch (v.step) {
      case "welcome":
        return page(
          this.footer(t, v.hints, this.keyButton(t, "quit", "Quit", "escape"), this.keyButton(t, "continue", "Get started", "enter", "primary")),
          v_flex().gap(8)
            .child(div().text_size(18).font_semibold().child(v.heading))
            .child(v_flex().gap(2).text_color(t.muted).child(v.intro).child(v.explanation)),
          h_flex().gap(8).items_center().text_size(12).child(div().text_color(t.dim).child(v.config_label)).child(path(v.config_path)));
      case "directories":
        return page(
          v.message
            ? h_flex().flex_none().h(FOOTER).px(EDGE).gap(10).items_center().border_t_1().border_color(t.border).text_color(t.blocked)
              .child(div().size(7).rounded(4).bg(t.blocked)).child(v.message)
            : this.footer(t, v.hints, v.depth
              ? this.keyButton(t, "add", "Add directory", "enter", "primary")
              : this.keyButton(t, "done", v.added.length && !v.path.query.before ? "Continue" : "Add", "enter", "primary")),
          div().text_color(t.muted).child(v.instructions),
          v_flex().gap(8)
            .child(label(v.path.label))
            .child(v.depth
              ? h_flex().h(38).px(INSET).gap(ICON_GAP).items_center().rounded(8).border_1().border_color(t.border).bg(t.background).text_color(t.muted)
                .child(icon(t, "folder", t.dim, ICON)).child(v.path.query.before + v.path.query.after)
              : field(t, v.path.query, v.path.placeholder))
            .when(!v.depth && v.completions.length > 0, (el) => el.child(v_flex().gap(2).children(v.completions.map((completion, i) =>
              h_flex().id(`kiosk-completion-${i}`).px(INSET).h(ROW).gap(ICON_GAP).rounded(7).items_center().cursor_pointer().text_size(13).text_color(t.muted)
                .when(i === v.selected_completion, (row) => row.bg(selectedFill(t)).text_color(t.foreground))
                .when(i !== v.selected_completion, (row) => row.hover((s) => s.bg(t.hover)))
                .on_click(() => this.send({ t: "completion", index: i }))
                .child(icon(t, "folder", i === v.selected_completion ? t.accent : t.dim, ICON))
                .child(completion))))),
          ...(v.depth ? [v_flex().gap(8).child(label(v.depth.label)).child(field(t, v.depth.query, v.depth.placeholder))] : []),
          v_flex().gap(6)
            .child(label(v.added_title))
            .child(v.added.length ? added(v.added) : div().text_size(12.5).text_color(t.dim).child(v.nothing_added)));
      case "folder_mode":
        return page(
          this.footer(t, v.hints, this.keyButton(t, "back", "Back", "escape"), this.keyButton(t, "confirm", "Confirm", "enter", "primary")),
          v_flex().gap(6)
            .child(heading(t, v.question))
            .child(h_flex().gap(6).items_center().text_size(12).text_color(t.dim).child(icon(t, "folder", t.dim, 12)).child(v.directory).child("·").child(v.depth)),
          v_flex().gap(8).children(v.options.map((option) =>
            h_flex().id(`kiosk-mode-${option.mode}`).px(INSET).h(42).gap(ICON_GAP).rounded(8).items_center().cursor_pointer().border_1()
              .border_color(option.selected ? tint(t.accent, 0.7) : t.border)
              .when(option.selected, (el) => el.bg(tint(t.accent, 0.08)))
              .when(!option.selected, (el) => el.hover((s) => s.bg(t.hover)))
              .on_click(() => this.send({ t: "folder_mode", mode: option.mode }))
              .child(div().size(ICON).flex_none().rounded(8).border_1().border_color(option.selected ? t.accent : t.input).flex().items_center().justify_center()
                .when(option.selected, (el) => el.bg(t.accent).child(div().size(6).rounded(3).bg(t.accent_foreground))))
              .child(option.label))));
      case "confirm":
        return page(
          this.footer(t, v.hints, this.keyButton(t, "back", "Back", "escape"), this.keyButton(t, "write", "Save and open", "enter", "primary")),
          heading(t, v.heading),
          added(v.dirs),
          h_flex().gap(8).items_center().text_size(12).child(div().text_color(t.dim).child(v.write_label)).child(path(v.config_path)));
      default:
        throw new Error(`the picker sent a setup step this mod doesn't know: ${v.step}`);
    }
  }

  noSearchDirs(t, v) {
    return panel(t)
      .child(this.titleBar(t, TITLE))
      .child(v_flex().items_center().justify_center().gap(8).py(32)
        .child(icon(t, "folder", t.dim, 20))
        .child(div().pt(4).child(heading(t, v.title)))
        .child(div().text_size(12.5).text_color(t.muted).child(v.location))
        .child(div().text_size(12).text_color(t.dim).child(v.hint)));
  }
}
