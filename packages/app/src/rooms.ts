// The room chip and the room browser: which room am I in, and how do I get
// to another one.
//
// Two surfaces, one always on screen:
//
//   * #roomchip — a small always-visible plate in the top-right corner, in
//     the HUD's own chrome language. It names the room, shows its code and
//     how many people are in it, and clicking it opens the browser. It is
//     the answer to "I have no idea what room I'm in", and it is deliberately
//     NOT part of #hud (which is pointer-events:none and rebuilt from a
//     string every frame — you cannot click a string).
//
//   * #roomdlg — the browser: three panes behind one modal.
//       ROOMS     every room on the server: join, rename, delete.
//       NEW ROOM  the templates the SERVER advertises, each with the blurb
//                 and the counts that say what you are actually starting
//                 (parts, panels, scope channels, whether it has a machine).
//       TEMPLATE  save the room you are standing in as a new template.
//
// Everything here talks plain HTTP to /api (see crates/server/src/lobby.rs).
// The lobby has to work BEFORE you have a room socket — "which rooms exist?"
// is the question you ask when you are not in one — so it is not part of the
// websocket protocol.
//
// Room identity: a 6-char CODE is the room (immutable, the filename, the
// ?room= value); the NAME is a label and can be changed by anyone. The chip
// shows both, because only one of them is the thing you paste to a friend.

import type { GoneReason, RoomHello, RoomView } from './net';
import type { SeedScope } from './scope';
import { lsFlag, lsGet, lsSet } from './store';

/** One row of GET /api/rooms. */
export interface RoomListing {
  id: string;
  name: string;
  template: string;
  parts: number;
  players: number;
  /** A room with a sim task running. Empty rooms park after 30 s and resume
   * on join — parked is normal, not broken, so it reads as "asleep". */
  live: boolean;
  machine: boolean;
  created: number;
  played: number;
}

/** One card of GET /api/templates. */
export interface TemplateListing {
  id: string;
  name: string;
  blurb: string;
  /** "builtin" ships with the server; "file" was saved into $EE_TEMPLATES
   * (and can be deleted from here). */
  source: string;
  parts: number;
  panels: number;
  probes: number;
  scopes: number;
  /** "none" | "hoist" — whether the room comes with a machine and a goal. */
  machine: string;
}

export interface RoomsDeps {
  /** Switch this client to a room; null = the server's default. */
  join(code: string | null): void;
  /** THIS client's camera rect and in-place scopes — the half of a room
   * setup the server does not own, handed over when saving a template. */
  view(): RoomView;
  /** Announce something in the world (main.ts's toast strip). */
  toast(msg: string): void;
}

export interface RoomsUI {
  /** A hello landed (or a pre-rooms server answered, with `room` null). */
  onHello(room: RoomHello | null): void;
  onPresence(n: number): void;
  onRoomMeta(id: string, name: string): void;
  /** The room went away under us. Returns the code to fall back to, having
   * already told the player why. */
  onGone(id: string, reason: GoneReason): void;
  /** The socket dropped: the chip stops claiming a live room. */
  onOffline(): void;
  open(tab?: Tab): void;
  close(): void;
  isOpen(): boolean;
  /** True while the event target is inside the room UI — the typing guard,
   * so 'r' in the name field does not arm a resistor. */
  owns(t: EventTarget | null): boolean;
  current(): RoomHello | null;
  /** The HUD's room segment, e.g. `room "Hoist practice" · K7QM2X`. */
  hudLabel(): string;
}

type Tab = 'rooms' | 'new' | 'save';

// ------------------------------------------------------------------- http

class ApiError extends Error {
  constructor(
    readonly code: string,
    readonly hint: string,
    readonly status: number,
  ) {
    super(hint || code);
  }
}

/** Every /api call funnels through here so a failure always has a code the
 * dialog can show, and a network drop reads as a message rather than an
 * unhandled rejection. */
async function api<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      headers: body === undefined ? undefined : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError('offline', 'no answer from the server', 0);
  }
  let json: Record<string, unknown> = {};
  try {
    json = (await res.json()) as Record<string, unknown>;
  } catch {
    /* an empty or non-JSON body is only a problem if the call failed */
  }
  if (!res.ok) {
    const code = typeof json.error === 'string' ? json.error : `http ${res.status}`;
    const hint = typeof json.hint === 'string' ? json.hint : '';
    throw new ApiError(code, hint, res.status);
  }
  return json as T;
}

// -------------------------------------------------------------- small DOM

const el = <K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  parent?: HTMLElement,
  text?: string,
): HTMLElementTagNameMap[K] => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  parent?.append(e);
  return e;
};

const button = (cls: string, label: string, parent: HTMLElement, run: () => void) => {
  const b = el('button', cls, parent, label);
  b.type = 'button';
  b.onclick = (ev) => {
    ev.stopPropagation();
    run();
  };
  return b;
};

/** "2 minutes ago" — rooms are sorted by last played, so the age is the
 * column that tells you which one you were actually in. */
function ago(unixSec: number): string {
  if (!Number.isFinite(unixSec) || unixSec <= 0) return 'never';
  const s = Math.max(0, Math.floor(Date.now() / 1000 - unixSec));
  if (s < 60) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// ------------------------------------------------------------------ build

export function createRooms(deps: RoomsDeps): RoomsUI {
  let room: RoomHello | null = null;
  let online = false;
  let tab: Tab = 'rooms';
  let rooms: RoomListing[] = [];
  let templates: TemplateListing[] = [];
  /** Template selected in the NEW pane; null until the list loads. */
  let pickedTemplate: string | null = null;
  /** Room whose delete is armed, waiting for the second click. */
  let armedDelete: string | null = null;
  /** Template id whose delete is armed. */
  let armedTemplateDelete: string | null = null;
  /** Room being renamed inline. */
  let renaming: string | null = null;
  /** Consecutive roomgone frames: the second one means "there is nowhere to
   * fall back to", so stop bouncing and show the browser instead. */
  let goneStreak = 0;
  /** The last lobby fetch failed. An empty list because the server is down
   * must not read as "this server has no rooms" — that would invite the
   * player to create one against a server that cannot answer. */
  let unreachable = false;

  // ---- the chip -----------------------------------------------------
  const chip = el('div', 'off', document.body);
  chip.id = 'roomchip';
  chip.tabIndex = 0;
  chip.setAttribute('role', 'button');
  chip.title = 'which room you are in — click to switch, create or delete rooms (⇧R)';
  el('span', 'rc-dot', chip);
  const chipName = el('span', 'rc-name', chip);
  const chipCode = el('span', 'rc-code', chip);
  const chipPop = el('span', 'rc-pop', chip);
  chip.onclick = () => (isOpen() ? close() : open('rooms'));
  chip.onkeydown = (ev) => {
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      open('rooms');
    }
  };

  function paintChip() {
    chip.classList.toggle('off', !online);
    chipName.textContent = room ? room.name : online ? 'this server has no room list' : 'offline';
    // The HUD writes `room "Name" · CODE`; the chip is the same sentence with
    // the same separator, so they read as one fact in two places.
    chipCode.textContent = room ? `· ${room.id}` : '';
    const n = room?.players ?? 0;
    chipPop.textContent = online && room ? `· ${n === 1 ? 'alone' : `${n} here`}` : '';
  }

  // ---- the dialog ---------------------------------------------------
  const back = el('div', '', document.body);
  back.id = 'roomdlg';
  const box = el('div', 'rdlg', back);
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-modal', 'true');
  box.setAttribute('aria-label', 'rooms');

  const hd = el('div', 'rdlg-hd', box);
  el('h3', '', hd, 'ROOMS');
  const tabs = el('div', 'rtabs', hd);
  const tabBtns: Record<Tab, HTMLButtonElement> = {
    rooms: button('rtab', 'browse', tabs, () => show('rooms')),
    new: button('rtab', 'new room', tabs, () => show('new')),
    save: button('rtab', 'save as template', tabs, () => show('save')),
  };
  button('rdlg-x', '×', hd, () => close()).title = 'close (Esc)';

  const body = el('div', 'rdlg-body', box);
  const foot = el('div', 'rdlg-ft', box);
  const msg = el('div', 'rmsg', foot);

  type MsgKind = '' | 'good' | 'bad';

  /** A message that has to outlive the fetch already in flight behind it.
   * The browser opens ITSELF when a room disappears under the player, and
   * opening it starts an async refresh whose routine "N rooms on this
   * server" lands a moment later — directly on top of the one sentence that
   * explains why a dialog just appeared. Pinned text holds the footer until
   * the PLAYER does something that deserves a new line. */
  let pinned: { text: string; kind: MsgKind } | null = null;

  function setMsg(text: string, kind: MsgKind) {
    msg.className = `rmsg${kind ? ` ${kind}` : ''}`;
    msg.textContent = text;
  }

  /** Say something because of what the player just did: it replaces a pin. */
  function say(text: string, kind: MsgKind = '') {
    pinned = null;
    setMsg(text, kind);
  }

  /** Say something because of what just HAPPENED TO the player. */
  function pin(text: string, kind: MsgKind) {
    pinned = { text, kind };
    setMsg(text, kind);
  }

  /** Routine status — a count, a hint. Never displaces an explanation. */
  function status(text: string) {
    if (pinned) setMsg(pinned.text, pinned.kind);
    else setMsg(text, '');
  }

  function fail(e: unknown) {
    const a = e instanceof ApiError ? e : null;
    say(a ? `${a.code}${a.hint ? ` — ${a.hint}` : ''}` : String(e), 'bad');
  }

  // ---- ROOMS pane ---------------------------------------------------

  function paintRooms() {
    body.innerHTML = '';
    foot.querySelectorAll('.rft').forEach((n) => n.remove());
    if (rooms.length === 0) {
      el(
        'div',
        'rempty',
        body,
        unreachable
          ? 'the server is not answering — the room list will come back when it does.'
          : 'no rooms on this server yet — start one from a template.',
      );
      if (!unreachable) button('rbtn go rft', 'new room…', foot, () => show('new'));
      return;
    }
    for (const r of rooms) {
      const here = room !== null && r.id === room.id;
      const row = el('div', `rrow${here ? ' here' : ''}`, body);

      const go = el('button', 'rjoin', row);
      go.type = 'button';
      go.disabled = here;
      const line = el('div', 'rn', go);
      el('span', 'rnm', line, r.name);
      if (here) el('span', 'rhere', line, 'YOU ARE HERE');
      if (r.machine) el('span', 'rtag', line, 'GOAL');
      el('div', 'rmeta', go).textContent =
        `${r.id} · from ${r.template} · ${plural(r.parts, 'part')} · ` +
        (r.players > 0 ? plural(r.players, 'player') : 'empty') +
        ` · ${r.live ? 'running' : 'asleep'} · played ${ago(r.played)}`;
      go.onclick = () => {
        close();
        deps.join(r.id);
      };

      const act = el('div', 'ract', row);
      if (renaming === r.id) {
        const input = el('input', 'rinput', act);
        input.type = 'text';
        input.value = r.name;
        input.maxLength = 40;
        input.setAttribute('aria-label', 'room name');
        const commit = () => void rename(r.id, input.value);
        input.onkeydown = (ev) => {
          if (ev.key === 'Enter') commit();
          else if (ev.key === 'Escape') {
            ev.stopPropagation();
            renaming = null;
            paintRooms();
          }
        };
        button('rbtn go', 'ok', act, commit);
        setTimeout(() => {
          input.focus();
          input.select();
        }, 0);
      } else if (armedDelete === r.id) {
        // Two-step, in place: destructive and irreversible, so it says what
        // it will take with it before it takes it.
        el('span', 'rwarn', act).textContent =
          r.players > 0 ? `delete? ${plural(r.players, 'player')} inside` : 'delete for good?';
        button('rbtn danger', 'yes, delete', act, () => void destroy(r));
        button('rbtn', 'cancel', act, () => {
          armedDelete = null;
          paintRooms();
        });
      } else {
        button('rbtn', 'rename', act, () => {
          renaming = r.id;
          armedDelete = null;
          paintRooms();
        });
        button('rbtn danger', 'delete', act, () => {
          armedDelete = r.id;
          renaming = null;
          paintRooms();
        });
      }
    }
    button('rbtn go rft', 'new room…', foot, () => show('new'));
  }

  async function refreshRooms() {
    try {
      const r = await api<{ rooms: RoomListing[] }>('/api/rooms');
      rooms = r.rooms ?? [];
      unreachable = false;
      if (tab === 'rooms') paintRooms();
      status(`${plural(rooms.length, 'room')} on this server`);
    } catch (e) {
      rooms = [];
      unreachable = true;
      if (tab === 'rooms') paintRooms();
      fail(e);
    }
  }

  async function rename(code: string, name: string) {
    const want = name.trim();
    renaming = null;
    if (!want) {
      paintRooms();
      return;
    }
    try {
      await api(`/api/rooms/${code}`, 'PATCH', { name: want });
      // The name we show comes from the roommeta broadcast, not from here —
      // one source of truth, and every other open tab updates with us.
      say(`renamed to "${want}"`, 'good');
      await refreshRooms();
    } catch (e) {
      fail(e);
      paintRooms();
    }
  }

  async function destroy(r: RoomListing) {
    armedDelete = null;
    try {
      await api(`/api/rooms/${r.id}`, 'DELETE');
      deps.toast(`room "${r.name}" (${r.id}) deleted`);
      // If it was OUR room, the server is already sending us a roomgone —
      // onGone does the moving. Otherwise just refresh the list.
      await refreshRooms();
    } catch (e) {
      fail(e);
      paintRooms();
    }
  }

  // ---- NEW pane -----------------------------------------------------

  let nameInput: HTMLInputElement | null = null;

  function paintNew() {
    body.innerHTML = '';
    foot.querySelectorAll('.rft').forEach((n) => n.remove());
    nameInput = null;
    if (templates.length === 0) {
      el(
        'div',
        'rempty',
        body,
        unreachable
          ? 'the server is not answering — no templates to offer.'
          : 'no templates on this server.',
      );
      return;
    }
    el('div', 'rlead', body).textContent =
      'A template is a whole room setup, not just a netlist: the parts, the ' +
      'control panels, the scope channels, where the camera lands, and ' +
      'whether the room comes with a machine to drive.';
    const grid = el('div', 'tgrid', body);
    for (const t of templates) {
      const card = el('button', `tcard${t.id === pickedTemplate ? ' on' : ''}`, grid);
      card.type = 'button';
      const top = el('div', 'tn', card);
      el('span', '', top, t.name);
      el('span', 'tsrc', top, t.source === 'file' ? 'saved' : 'built-in');
      el('div', 'tb', card, t.blurb || 'No description.');
      const bits = [
        plural(t.parts, 'part'),
        t.panels > 0 ? plural(t.panels, 'panel') : '',
        t.probes > 0 ? `${plural(t.probes, 'scope channel')}` : '',
        t.scopes > 0 ? plural(t.scopes, 'scope') : '',
        t.machine !== 'none' ? `machine: ${t.machine}` : 'no machine',
      ].filter(Boolean);
      el('div', 'ts', card, bits.join(' · '));
      card.onclick = () => {
        pickedTemplate = t.id;
        // A name the player already typed is theirs and survives the repaint;
        // an untouched field goes on tracking the template as a placeholder,
        // so "create" is one more click from here and nothing is mandatory.
        const typed = nameInput?.dataset.touched === '1' ? nameInput.value : null;
        paintNew();
        if (typed !== null && nameInput) {
          nameInput.value = typed;
          nameInput.dataset.touched = '1';
        }
      };
      if (t.source === 'file') {
        const x = button('tdel', '×', card, () => {
          armedTemplateDelete = armedTemplateDelete === t.id ? null : t.id;
          paintNew();
        });
        x.title = 'delete this saved template';
        if (armedTemplateDelete === t.id) {
          const conf = el('div', 'tconfirm', card);
          el('span', '', conf, 'delete template?');
          button('rbtn danger', 'yes', conf, () => void deleteTemplate(t.id));
        }
      }
    }

    const picked = templates.find((t) => t.id === pickedTemplate) ?? null;
    const bar = el('div', 'rft rmake', foot);
    const input = el('input', 'rinput wide', bar);
    input.type = 'text';
    input.maxLength = 40;
    input.placeholder = picked ? picked.name : 'room name';
    input.setAttribute('aria-label', 'new room name');
    input.oninput = () => (input.dataset.touched = '1');
    input.onkeydown = (ev) => {
      if (ev.key === 'Enter') void create(input.value);
    };
    nameInput = input;
    const go = button('rbtn go', 'create room', bar, () => void create(input.value));
    go.disabled = picked === null;
    status(picked ? `new room from "${picked.name}"` : 'pick a template');
  }

  async function refreshTemplates() {
    try {
      const r = await api<{ templates: TemplateListing[] }>('/api/templates');
      templates = r.templates ?? [];
      unreachable = false;
      if (pickedTemplate === null || !templates.some((t) => t.id === pickedTemplate)) {
        pickedTemplate = templates[0]?.id ?? null;
      }
      if (tab === 'new') paintNew();
    } catch (e) {
      templates = [];
      unreachable = true;
      if (tab === 'new') paintNew();
      fail(e);
    }
  }

  async function create(name: string) {
    if (!pickedTemplate) return;
    say('creating…');
    try {
      const r = await api<{ room: RoomListing }>('/api/rooms', 'POST', {
        name: name.trim(),
        template: pickedTemplate,
      });
      close();
      deps.toast(`created "${r.room.name}" (${r.room.id})`);
      deps.join(r.room.id);
    } catch (e) {
      fail(e);
    }
  }

  async function deleteTemplate(id: string) {
    armedTemplateDelete = null;
    try {
      await api(`/api/templates/${id}`, 'DELETE');
      say(`template "${id}" deleted`, 'good');
      await refreshTemplates();
    } catch (e) {
      fail(e);
      paintNew();
    }
  }

  // ---- SAVE-AS-TEMPLATE pane ----------------------------------------

  function paintSave() {
    body.innerHTML = '';
    foot.querySelectorAll('.rft').forEach((n) => n.remove());
    if (!room) {
      el('div', 'rempty', body, 'not in a room — join one first.');
      return;
    }
    const here = room;
    el('div', 'rlead', body).textContent =
      `Save "${here.name}" (${here.id}) as a template, so a new room can start ` +
      'from exactly this setup.';
    el('div', 'rlabel', body, 'it keeps');
    const keep = el('ul', 'rkeep', body);
    el('li', '', keep, 'the parts, wires and values as they stand right now');
    el('li', '', keep, 'the control panels and the scope channels this room has armed');
    el('li', '', keep, 'your camera and your in-place oscilloscopes as the starting view');
    if (here.machine) el('li', '', keep, 'the machine and where its cabinet was dragged to');
    el('div', 'rlabel', body, 'it strips, so the next player starts clean');
    const drop = el('ul', 'rdrop', body);
    el('li', '', drop, 'burnt-out parts are put back into service');
    if (here.machine) el('li', '', drop, 'the goal is re-armed — a template must not ship a won game');

    const form = el('div', 'rform', body);
    const idRow = el('label', 'rfield', form);
    el('span', '', idRow, 'template id');
    const idIn = el('input', 'rinput', idRow);
    idIn.type = 'text';
    idIn.maxLength = 32;
    idIn.placeholder = 'my-lab';
    idIn.value = suggestId(here.name);

    const nameRow = el('label', 'rfield', form);
    el('span', '', nameRow, 'name');
    const nameIn = el('input', 'rinput', nameRow);
    nameIn.type = 'text';
    nameIn.maxLength = 60;
    nameIn.value = here.name;

    const blurbRow = el('label', 'rfield', form);
    el('span', '', blurbRow, 'blurb');
    const blurbIn = el('input', 'rinput wide', blurbRow);
    blurbIn.type = 'text';
    blurbIn.maxLength = 160;
    blurbIn.placeholder = 'what a player is starting when they pick this';

    const overRow = el('label', 'rfield rcheck', form);
    const over = el('input', '', overRow);
    over.type = 'checkbox';
    el('span', '', overRow, 'overwrite an existing template with that id');

    const run = () =>
      void saveTemplate(here.id, idIn.value, nameIn.value, blurbIn.value, over.checked);
    for (const i of [idIn, nameIn, blurbIn]) {
      i.onkeydown = (ev) => {
        if (ev.key === 'Enter') run();
      };
    }
    button('rbtn go rft', 'save template', foot, run);
    status('the template is written on the server, and appears in "new room" at once');
  }

  /** A room name → a legal template id (`^[a-z0-9][a-z0-9-]{0,31}$`). */
  function suggestId(name: string): string {
    const s = name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 32);
    return s || 'my-room';
  }

  async function saveTemplate(
    from: string,
    id: string,
    name: string,
    blurb: string,
    overwrite: boolean,
  ) {
    say('saving…');
    try {
      const r = await api<{ template: TemplateListing }>('/api/templates', 'POST', {
        from,
        id: id.trim().toLowerCase(),
        name: name.trim(),
        blurb: blurb.trim(),
        // The camera and the in-place scopes are CLIENT state — no one else
        // has them, so they travel in this request.
        view: deps.view(),
        overwrite,
      });
      deps.toast(`saved template "${r.template?.name ?? id}"`);
      await refreshTemplates();
      show('new');
      pickedTemplate = r.template?.id ?? id.trim().toLowerCase();
      paintNew();
      say('saved — pick it to start a room from it', 'good');
    } catch (e) {
      fail(e);
    }
  }

  // ---- pane switching + keyboard ------------------------------------

  function show(next: Tab) {
    tab = next;
    armedDelete = null;
    armedTemplateDelete = null;
    renaming = null;
    for (const k of Object.keys(tabBtns) as Tab[]) {
      tabBtns[k].classList.toggle('on', k === tab);
    }
    say('');
    // Focus AFTER the list lands, not before: the pane starts empty while the
    // fetch is in flight, and focusing then would park the caret on the tab
    // strip instead of on the first room the player can actually pick.
    if (tab === 'rooms') {
      paintRooms();
      void refreshRooms().then(focusFirst);
    } else if (tab === 'new') {
      paintNew();
      void refreshTemplates().then(focusFirst);
    } else {
      paintSave();
      focusFirst();
    }
  }

  function focusables(): HTMLElement[] {
    return [...box.querySelectorAll<HTMLElement>('button:not([disabled]), input')].filter(
      (n) => n.offsetParent !== null,
    );
  }

  function focusFirst() {
    // The list itself, not the tab strip: the room you want is one Enter away.
    const first = body.querySelector<HTMLElement>('button:not([disabled]), input');
    setTimeout(() => (first ?? tabBtns[tab]).focus(), 0);
  }

  const isOpen = () => back.classList.contains('open');

  function open(next: Tab = 'rooms') {
    back.classList.add('open');
    show(next);
  }

  function close() {
    back.classList.remove('open');
    armedDelete = null;
    armedTemplateDelete = null;
    renaming = null;
  }

  back.onpointerdown = (ev) => {
    if (ev.target === back) close(); // click the backdrop to dismiss
  };

  // The dialog owns its own keys and stops them at the boundary: the world
  // below is a canvas whose every letter is a tool.
  back.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation();
      ev.preventDefault();
      close();
      return;
    }
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      const list = focusables();
      if (list.length === 0) return;
      const at = list.indexOf(document.activeElement as HTMLElement);
      const step = ev.key === 'ArrowDown' ? 1 : -1;
      const next = list[(at + step + list.length) % list.length];
      next?.focus();
      ev.stopPropagation();
      ev.preventDefault();
      return;
    }
    // Everything else (typing, Tab, Enter on a button) is the dialog's.
    ev.stopPropagation();
  });

  // ---- the room the client is actually in ---------------------------

  function adopt(next: RoomHello | null) {
    room = next;
    online = true;
    goneStreak = 0;
    paintChip();
    document.title = next ? `EE Game — ${next.name}` : 'EE Game';
    if (next) {
      // Every tab becomes an invite link. replaceState, not pushState: a room
      // switch is not a navigation, and Back must not walk into a room the
      // client is no longer connected to.
      const url = `${location.pathname}?room=${encodeURIComponent(next.id)}${location.hash}`;
      try {
        history.replaceState(null, '', url);
      } catch {
        /* file:// and sandboxed frames refuse; the chip still says where we are */
      }
    }
    if (isOpen() && tab === 'rooms') paintRooms();
  }

  // Before the first hello there is no room yet, and the chip says exactly
  // that ("offline") rather than sitting blank — the whole point of it is
  // that the player is never left guessing where they are.
  paintChip();

  return {
    onHello(next) {
      adopt(next);
    },
    onPresence(n) {
      if (room) {
        room = { ...room, players: n };
        paintChip();
      }
    },
    onRoomMeta(id, name) {
      if (room && room.id === id) {
        room = { ...room, name };
        paintChip();
        document.title = `EE Game — ${name}`;
      }
      const r = rooms.find((x) => x.id === id);
      if (r) r.name = name;
      if (isOpen() && tab === 'rooms') paintRooms();
    },
    onGone(id, reason) {
      const was = room?.name ?? id;
      room = null;
      online = false;
      paintChip();
      document.title = 'EE Game';
      goneStreak++;
      // A server with NO rooms at all answers every reconnect with another
      // roomgone — one per RECONNECT_MS, for as long as the player sits
      // there. So this runs on a repeat, and everything it does has to be
      // something it is willing to do forever:
      //
      //   1st  news. Say it, and fall back to whatever the server calls
      //        default — which is nearly always where the player wants to be.
      //   2nd  the answer: there is nowhere to fall back to. Show the browser
      //        and explain, once.
      //   3rd+ the player is ALREADY looking at the browser, quite possibly
      //        half-way through naming their new room. Say nothing, touch
      //        nothing. Re-opening the browser here is what put the tab back
      //        to "browse" and binned a half-typed name every 2.5 s.
      if (goneStreak <= 2) {
        if (reason === 'deleted') deps.toast(`room "${was}" was deleted — moving you out`);
        else deps.toast(`room ${id || '?'} is not on this server`);
      }
      if (goneStreak === 1) {
        deps.join(null);
        return;
      }
      if (goneStreak > 2) return;
      // Open the browser — unless it is already open, in which case it is the
      // player's: refresh the list under them rather than resetting the pane
      // they are working in.
      if (!isOpen()) open('rooms');
      else if (tab === 'rooms') void refreshRooms();
      // Pinned, not said: `open()` has just started a room fetch whose
      // "0 rooms on this server" would otherwise land on top of the only
      // sentence that explains why this dialog is here.
      pin('that room is gone — join another, or start a new one', 'bad');
    },
    onOffline() {
      online = false;
      paintChip();
    },
    open,
    close,
    isOpen,
    owns: (t) => t instanceof Node && (back.contains(t) || chip.contains(t)),
    current: () => room,
    hudLabel: () => (room ? `room "${room.name}" · ${room.id}` : ''),
  };
}

// ------------------------------------------------- the bench, per room code
//
// In-place scopes are client-local: main.ts owns the array, the server has
// never seen a sid, and nothing about them is replicated. So a template SEEDS
// them — the room hands over a rect, a channel list and a timebase, and from
// then on they are that player's own instruments to move, retune or delete.
//
// Which makes them the only piece of a room that exists NOWHERE if this
// browser does not keep it. A flag saying "already seeded" is durable; the
// instruments it gated were not, so the first reload after a join left the
// bench permanently empty — seeded once, then gone forever, with the flag
// still set so they could never come back.
//
// The fix is to persist the thing itself rather than a proxy for it. The
// stored LIST is now the record: its presence means "this browser has been in
// this room" (so a template never re-litters the bench), and its contents are
// the bench itself (so a reload puts the instruments back exactly where the
// player left them, not where the template first put them). One record, so
// the two can never disagree — which the flag-plus-memory pair did, silently,
// the moment the page reloaded.

const seedKey = (code: string) => `ee.room.${code}.seeded`;
const benchKey = (code: string) => `ee.room.${code}.scopes`;

/**
 * The in-place scopes this browser has in room `code`, or null when it has
 * never been there — the caller's cue to materialize the template's seeds.
 *
 * An empty array is NOT null: "I have been here and I have no scopes" is a
 * player who closed them, and re-seeding that bench would be the litter this
 * whole mechanism exists to prevent.
 *
 * Every element is untrusted (localStorage is user-editable and survives
 * across versions); `seedToScope` clamps each one, so all this owes the
 * caller is "an array of objects, or null".
 */
export function loadBench(code: string): SeedScope[] | null {
  const isSeed = (s: unknown): s is SeedScope =>
    !!s && typeof s === 'object' && !Array.isArray(s);
  const raw = lsGet(benchKey(code));
  if (raw === null) {
    // Migration: profiles that joined under the flag-only build. The flag is
    // the only surviving evidence that they were here, and honouring it keeps
    // the promise it was written for — no re-litter — at the cost of the
    // scopes that build had already lost for them.
    return lsFlag(seedKey(code)) ? [] : null;
  }
  try {
    const v: unknown = JSON.parse(raw);
    // The same deal net.ts makes with `view.scopes`: keep the objects, drop
    // everything else, and let `seedToScope` clamp what is inside them.
    return Array.isArray(v) ? v.filter(isSeed) : [];
  } catch {
    return [];
  }
}

/** Record this browser's bench for room `code`. Called with an empty list
 * too: that is what "been here, closed them all" looks like. */
export function saveBench(code: string, scopes: unknown[]): void {
  lsSet(benchKey(code), JSON.stringify(scopes));
  // Keep the legacy flag in step so a downgrade does not re-seed the bench.
  lsSet(seedKey(code), '1');
}
