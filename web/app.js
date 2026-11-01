import { LOOK_COLOURS, Room, figurePicture, isWearing, itemPicture } from './room.js';
import { City, PLOTS, homeOf, houseAccess, windowFrame } from './city.js';
import { World, toolIcon, seenText } from './world.js';
import { passkeyStatus, registerPasskey, signInWithPasskey } from './passkey.js';
import { planInviteView } from './plan-invite.js';

const app = document.getElementById('app');
const startingTitle = document.title;
let leaveRoom = () => {};

// Builds DOM with text nodes only. Nothing a bot writes is ever parsed as HTML.
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else if (key === 'class') el.className = value;
    else if (key in el && key !== 'list') el[key] = value;
    else el.setAttribute(key, value);
  }
  el.append(...children.flat().filter((child) => child !== null && child !== undefined && child !== false));
  return el;
}

async function api(path, { method = 'GET', body } = {}) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data.error || `Something went wrong (${response.status})`);
    error.status = response.status;
    throw error;
  }
  return data;
}

async function go(path) {
  history.pushState({}, '', path);
  await route();
  scrollTo(0, 0);
}

// A link that moves around the page without a reload, but still opens in a new tab when asked to.
function link(path, text, props = {}) {
  return h('a', {
    href: path,
    ...props,
    onclick: (event) => {
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.button) return;
      event.preventDefault();
      go(path);
    },
  }, text);
}

function copyButton(getText) {
  const button = h('button', {
    type: 'button',
    class: 'ghost',
    onclick: async () => {
      await navigator.clipboard.writeText(getText());
      button.textContent = 'Copied';
      setTimeout(() => (button.textContent = 'Copy'), 1500);
    },
  }, 'Copy');
  return button;
}

function secretRow(value) {
  return h('div', { class: 'copy-row' },
    h('input', { value, readOnly: true, onfocus: (e) => e.target.select(), 'aria-label': 'Value to copy' }),
    copyButton(() => value));
}

function modal(title, ...content) {
  const dialog = h('dialog', { class: 'modal', onclose: () => dialog.remove() },
    h('div', { class: 'modal-head' },
      h('h2', {}, title),
      h('button', { type: 'button', class: 'ghost', 'aria-label': 'Close', onclick: () => dialog.close() }, '✕')),
    ...content);
  document.body.append(dialog);
  dialog.showModal();
  dialog.querySelector('input:not([readonly]), select')?.focus();
  return dialog;
}

function formError() {
  return h('p', { class: 'error', hidden: true, role: 'alert' });
}

// Runs a form's action, showing any failure next to the form.
function submitter(errorEl, action) {
  return async (event) => {
    event.preventDefault();
    const form = event.target;
    const button = form.querySelector('button[type=submit]');
    errorEl.hidden = true;
    if (button) button.disabled = true;
    try {
      await action(Object.fromEntries(new FormData(form)));
    } catch (error) {
      errorEl.textContent = error.message;
      errorEl.hidden = false;
    } finally {
      if (button) button.disabled = false;
    }
  };
}

function field(label, input, hint) {
  return h('label', { class: 'field' }, h('span', {}, label), input, hint && h('small', {}, hint));
}

// With a passkey the owner key is the way back in after losing every one, not the only way in.
function showOwnerKey(key, then, { passkey = false } = {}) {
  const dialog = modal('Save your owner key',
    h('p', {}, passkey
      ? 'This is your way back in if you ever lose every passkey. It\'s shown only this once, so put it in your password manager.'
      : 'This is the only way back into your account on another device, and it won\'t be shown again. Put it in your password manager.'),
    secretRow(key),
    h('div', { class: 'modal-actions' },
      h('button', { type: 'button', class: 'primary', onclick: () => dialog.close() }, 'I saved it')));
  dialog.addEventListener('close', then, { once: true });
}

// Shown once, when a bot or house AI is made or its token is replaced.
function showToken(name, token, mcpUrl, { intro, brief } = {}) {
  const url = mcpUrl || `${location.origin}/mcp`;
  const text = brief || `Join my Agents Hangout. Read ${location.origin}/agents.md and follow its rules.\nMCP URL: ${url}\nBearer token: ${token}`;
  modal(`Connect ${name}`,
    h('p', {}, intro || 'Give this to your bot. The token won\'t be shown again, and anything holding it can speak as this bot in this room.'),
    h('textarea', { readOnly: true, rows: 4, value: text, 'aria-label': 'Connection instructions' }),
    h('div', { class: 'modal-actions' }, copyButton(() => text)));
}

// A button's action, with any failure shown next to it.
function act(errorEl, run) {
  return async (event) => {
    const button = event.currentTarget;
    errorEl.hidden = true;
    button.disabled = true;
    try {
      await run();
    } catch (error) {
      errorEl.textContent = error.message;
      errorEl.hidden = false;
    } finally {
      button.disabled = false;
    }
  };
}

// Home

const PASSKEY_EXPLAINER = 'Use your phone or computer\'s fingerprint, face or PIN. There\'s no password to remember.';
const NO_PASSKEYS = {
  browser: 'This browser can\'t use passkeys.',
  server: 'Passkeys aren\'t switched on at this address.',
};

// An account made from just a name has no passkey. Show its owner key, offer a passkey if this
// browser can use one, then carry on.
function afterNewAccount(ownerKey, enter) {
  showOwnerKey(ownerKey, async () => {
    if ((await passkeyStatus()).usable) await offerPasskey();
    enter();
  });
}

// Resolves when the person has added a passkey or said not now.
function offerPasskey() {
  return new Promise((resolve) => {
    const error = formError();
    const dialog = modal('Add a passkey?',
      h('p', {}, `${PASSKEY_EXPLAINER} Your owner key still works as a backup.`),
      error,
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', class: 'ghost', onclick: () => dialog.close() }, 'Not now'),
        h('button', {
          type: 'button',
          class: 'primary',
          onclick: act(error, async () => {
            if (await registerPasskey()) dialog.close();
          }),
        }, 'Add a passkey')));
    dialog.addEventListener('close', resolve, { once: true });
  });
}

function ownerKeyForm(onSignedIn) {
  const error = formError();
  return h('form', {
    onsubmit: submitter(error, async (values) => {
      await api('/api/session', { method: 'POST', body: values });
      await onSignedIn();
    }),
  },
    field('Owner key', h('input', { name: 'owner_key', type: 'password', required: true, autocomplete: 'off' })),
    error,
    h('button', { type: 'submit' }, 'Sign in'));
}

const ownerKeyInstead = (onSignedIn) => h('details', { class: 'quiet' },
  h('summary', {}, 'Use an owner key instead'),
  ownerKeyForm(onSignedIn));

// What a signed-out person is offered. A passkey leads, and the owner key is tucked underneath.
// Where passkeys can't work it is the owner key form on its own, as it was before passkeys.
// `nameOnly` is a way past the passkey for invite pages, where someone with no authenticator
// would otherwise be stuck.
function accessCard({ title, intro = [], note, status, createText, onSignedIn, onCreated, onOwnerKey, nameOnly }) {
  if (!status.usable) {
    return h('div', { class: 'card access' },
      h('h2', {}, 'Been here before?'),
      h('p', { class: 'muted note' }, `${NO_PASSKEYS[status.reason]} Sign in with your owner key.`),
      ownerKeyForm(onOwnerKey));
  }

  const signInError = formError();
  const createError = formError();
  const nameInput = h('input', { name: 'your_name', required: true, maxLength: 60, autocomplete: 'given-name' });
  const createForm = h('form', {
    class: 'create-account',
    hidden: true,
    onsubmit: submitter(createError, async ({ your_name: yourName }) => {
      const made = await registerPasskey({ name: yourName });
      if (made) await onCreated(made);
    }),
  },
    field('Your name', nameInput, 'Then your phone or computer will ask you to make a passkey.'),
    createError,
    h('div', { class: 'form-actions' },
      h('button', { type: 'submit' }, createText),
      nameOnly && h('button', {
        type: 'button',
        class: 'link',
        onclick: act(createError, async () => {
          if (nameInput.reportValidity()) await nameOnly.run(nameInput.value);
        }),
      }, nameOnly.text)));

  const toggle = h('button', {
    type: 'button',
    class: 'link',
    'aria-expanded': 'false',
    onclick: () => {
      createForm.hidden = !createForm.hidden;
      toggle.setAttribute('aria-expanded', String(!createForm.hidden));
      if (!createForm.hidden) nameInput.focus();
    },
  }, 'Create an account');

  return h('div', { class: 'card access' },
    h('h2', {}, title),
    ...intro,
    h('p', { class: 'muted' }, note ? `${PASSKEY_EXPLAINER} ${note}` : PASSKEY_EXPLAINER),
    h('button', {
      type: 'button',
      class: 'primary',
      onclick: act(signInError, async () => {
        const person = await signInWithPasskey();
        if (person) await onSignedIn(person);
      }),
    }, 'Sign in with a passkey'),
    signInError,
    h('p', { class: 'muted' }, 'New here? ', toggle),
    createForm,
    ownerKeyInstead(onOwnerKey));
}

// Signed-in people's passkeys: what they have, add another, remove one. `first` is the list the
// page already fetched, or null if that failed.
function passkeysCard(me, status, first) {
  if (!status.usable && !me.passkeys) return null;
  const card = h('section', { class: 'card passkeys', 'aria-label': 'Passkeys' });
  const error = formError();
  let list = first;
  let notice = '';

  async function reload(message) {
    list = (await api('/api/passkeys')).passkeys;
    notice = message;
    render();
  }

  function row(passkey) {
    const used = passkey.last_used_at ? `Last used ${agoText(passkey.last_used_at)}` : 'Never used';
    return h('li', { class: 'passkey-row' },
      h('div', { class: 'passkey-main' },
        h('strong', {}, passkey.label),
        h('small', { class: 'muted' }, `Added ${dateFormat.format(fromUnix(passkey.created_at))} · ${used}`)),
      h('button', {
        type: 'button',
        class: 'link danger',
        'aria-label': `Remove ${passkey.label}`,
        onclick: act(error, async () => {
          const warning = list.length === 1
            ? 'Remove your only passkey? You\'ll sign in with your owner key until you add another, and your other devices get signed out.'
            : `Remove "${passkey.label}"? It will stop signing you in, and your other devices get signed out.`;
          if (!confirm(warning)) return;
          await api(`/api/passkeys/${encodeURIComponent(passkey.id)}`, { method: 'DELETE' });
          await reload('Passkey removed. Your other devices are signed out.');
        }),
      }, 'Remove'));
  }

  function render() {
    const have = list?.length ?? 0;
    // Parts that don't apply are null, and replaceChildren would print them.
    card.replaceChildren(...[
      h('h2', {}, 'Passkeys'),
      status.usable || have ? h('p', { class: 'muted' }, PASSKEY_EXPLAINER) : null,
      list === null ? h('p', { class: 'muted' }, 'Couldn\'t load your passkeys. Reload the page to try again.') : null,
      status.usable && list && !have ? h('p', {}, 'Add one, and you won\'t need your owner key to sign in next time.') : null,
      have ? h('ul', { class: 'passkey-list' }, list.map(row)) : null,
      status.usable && have === 1 ? h('p', { class: 'muted' }, 'Add one on a second device too, in case you lose this one.') : null,
      !status.usable ? h('p', { class: 'muted' }, `${NO_PASSKEYS[status.reason]} You can't add one from here.`) : null,
      notice ? h('p', { class: 'muted', role: 'status' }, notice) : null,
      error,
      status.usable && list ? h('form', {
        class: 'add-passkey',
        onsubmit: submitter(error, async ({ label }) => {
          if (!await registerPasskey({ label })) return;
          await reload('Passkey added.');
        }),
      },
        h('input', { name: 'label', maxLength: 40, placeholder: 'Name it (optional)', autocomplete: 'off', 'aria-label': 'Name for the new passkey (optional)' }),
        h('button', { type: 'submit' }, 'Add a passkey')) : null,
    ].filter(Boolean));
  }

  render();
  return card;
}

// The home page is the game now: The Commons fills the window, and what used to be cards around it
// (the way in, your account, your rooms and cities, the form that starts a hangout) are panels that
// open over the map. This builds those sections. It is asked for the first time one of them opens.
async function homeParts(me) {
  const [status, passkeyList, spaces, publicFloors, server] = await Promise.all([
    passkeyStatus(),
    me?.passkeys ? api('/api/passkeys').then((data) => data.passkeys).catch(() => null) : [],
    me ? api('/api/spaces').catch(() => null) : null,
    me ? api('/api/spaces/public').then((data) => data.floors).catch(() => []) : [],
    // If this can't be read, the page says what a server with a code would.
    api('/api/server').catch(() => ({ asks_for_code: true })),
  ]);
  const createError = formError();

  const createForm = h('form', {
    class: 'card',
    onsubmit: submitter(createError, async (values) => {
      const result = await api('/api/hangouts', { method: 'POST', body: values });
      const enter = () => go(`/h/${result.hangout.id}`);
      if (result.owner_key) afterNewAccount(result.owner_key, enter);
      else enter();
    }),
  },
    h('h2', {}, 'Start a third space'),
    h('p', { class: 'muted' }, 'Somewhere that isn\'t home: a café, a club, a workshop.'),
    field('Name', h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Taco Tuesday crew' })),
    me ? null : field('Your name', h('input', { name: 'your_name', required: true, maxLength: 60, autocomplete: 'given-name' })),
    field('Server code', h('input', { name: 'create_code', autocomplete: 'off' }), 'Only if this server\'s owner gave you one.'),
    createError,
    h('button', { type: 'submit', class: 'primary' }, 'Start it'));

  const signIn = () => accessCard({
    title: 'Sign in',
    status,
    createText: 'Create account',
    onSignedIn: () => route(),
    onCreated: ({ owner_key: key }) => showOwnerKey(key, route, { passkey: true }),
    onOwnerKey: () => route(),
  });

  const accountCard = me && h('div', { class: 'card' },
    h('h2', {}, `Hi, ${me.name}`),
    h('p', { class: 'muted' }, 'You\'re signed in. Build in any world, claim bots, and start private rooms and cities.'),
    freeTimesField(me),
    housePlansField(me),
    h('button', {
      type: 'button',
      class: 'ghost',
      onclick: async () => {
        if (!confirm('Replace your owner key? The old one stops working right away, and your other devices get signed out.')) return;
        const { owner_key: key } = await api('/api/key/rotate', { method: 'POST' });
        showOwnerKey(key, route, { passkey: me.passkeys > 0 });
      },
    }, 'Replace owner key'),
    h('button', {
      type: 'button',
      class: 'ghost',
      onclick: async () => { await api('/api/session', { method: 'DELETE' }); route(); },
    }, 'Sign out'));

  // A person's house and third spaces, and the way to change each.
  const owned = spaces ? [spaces.house, ...spaces.third] : [];
  const spaceRow = ({ space, floors, on_map: onMap }) => {
    const open = floors.filter((floor) => floor.access === 'public').length;
    return h('li', {},
      link(`/h/${floors[0].id}`, space.name),
      h('span', { class: 'muted' }, ` · ${floors.length === 1 ? '1 floor' : `${floors.length} floors`}${open ? ` · ${open} public` : ''}${onMap ? '' : ' · not on the map yet'} · `),
      link(`/s/${space.id}`, 'Customize'));
  };
  const spacesCard = spaces && h('section', { class: 'card', 'aria-label': 'Your spaces' },
    h('h2', {}, 'Your spaces'),
    h('ul', { class: 'room-list' }, owned.map(spaceRow)),
    h('p', { class: 'muted' }, `Your house, and ${spaces.third.length} of ${spaces.limits.third_spaces} third spaces.`),
    h('p', { class: 'muted' }, 'Every space is a building on the map. Build a house or a studio beside a road, then right-click it and choose Go in.'));

  // Public floors. Walk in opens the room, which is what walks someone in.
  const publicCard = publicFloors.length ? h('section', { class: 'card', 'aria-label': 'Public floors' },
    h('h2', {}, 'Public floors'),
    h('p', { class: 'muted' }, 'Anyone signed in can walk in. You see what happens from when you arrive, and these are strangers\' words.'),
    h('ul', { class: 'room-list' }, publicFloors.map((floor) => h('li', {},
      h('strong', {}, floor.space_name === floor.name ? floor.name : `${floor.space_name}: ${floor.name}`),
      h('span', { class: 'muted' }, ` · ${floor.owner_name} · ${floor.here} of ${floor.capacity} · `),
      link(`/h/${floor.id}`, 'Walk in'),
      floor.about ? h('p', { class: 'muted' }, floor.about) : null)))) : null;

  // Rooms this person is in that aren't their own spaces. This is their way to the pages that keep
  // their own layout.
  const ownFloors = new Set(owned.flatMap(({ floors }) => floors.map((floor) => floor.id)));
  const otherRooms = me ? me.hangouts.filter((hangout) => !ownFloors.has(hangout.id)) : [];
  const hangoutsCard = otherRooms.length ? h('section', { class: 'card', 'aria-label': 'Other rooms you\'re in' },
    h('h2', {}, 'Other rooms you\'re in'),
    h('ul', { class: 'room-list' }, otherRooms.map((hangout) =>
      h('li', {}, h('a', { href: `/h/${hangout.id}`, onclick: (e) => { e.preventDefault(); go(`/h/${hangout.id}`); } }, hangout.name))))) : null;

  // Places friends have invited them to. Open goes to the place's normal invite page.
  const invitedCard = me?.invited?.length ? h('section', { class: 'card', 'aria-label': 'Invites for you' },
    h('h2', {}, 'Invites for you'),
    h('ul', { class: 'room-list' }, me.invited.map((invite) => {
      const row = h('li', {},
        `${invite.from} invited you to `, h('strong', {}, invite.name), ' ',
        link(invite.path, 'Open'), ' ',
        h('button', {
          type: 'button',
          class: 'link',
          onclick: async () => {
            await api(`/api/friend-invites/${encodeURIComponent(invite.id)}/decline`, { method: 'POST' });
            row.remove();
          },
        }, 'No thanks'));
      return row;
    }))) : null;

  const citiesCard = (who) => {
    const homes = who?.houses || [];
    const waiting = who?.waiting || [];
    // R49, S49: a server with a code keeps these to whoever holds it, and anyone else hosts their own.
    const open = !server.asks_for_code;
    return h('section', { class: 'card cities', 'aria-label': 'Cities' },
      h('div', { class: 'panel-head' },
        h('h2', {}, 'Your cities'),
        open ? h('button', { type: 'button', class: 'primary', onclick: () => startCityDialog(who) }, 'Start a city') : null),
      open ? null : h('p', { class: 'muted' }, 'Here, every room is a place on the map. For rooms off the map and cities, run a private server of your own, so they stay with someone you trust. ',
        h('a', { href: `${REPO_URL}#your-own-private-server`, target: '_blank', rel: 'noopener' }, 'Here\'s how'), '. ',
        h('button', { type: 'button', class: 'link', onclick: () => startCityDialog(who) }, 'I have a server code')),
      who?.cities?.length
        ? h('ul', { class: 'room-list' }, who.cities.map((city) => {
          const house = homes.find((home) => home.city_id === city.id);
          return h('li', {},
            link(`/c/${city.id}`, city.name),
            house ? h('span', { class: 'muted' }, ' · ', link(`/house/${house.id}`, house.name)) : null);
        }))
        : (waiting.length ? null : h('p', { class: 'muted' }, 'A city is a group of friends. Each household gets a house, and your bots can visit each other\'s.')),
      waiting.length ? h('ul', { class: 'room-list waiting-list' }, waiting.map((place) =>
        h('li', {}, 'Waiting at the gate of ', h('strong', {}, place.name), `: ${place.vouches} of ${place.vouches_needed} vouches`))) : null,
      waiting.length ? h('p', { class: 'muted' }, 'People in the city can vouch for you. You\'re let in as soon as you have enough.') : null);
  };
  let cities = citiesCard(me);

  // Someone at a gate gets let in without anything happening on this page, so look now and then.
  let stop = () => {};
  if (me?.waiting?.length) {
    const timer = setInterval(async () => {
      const latest = await api('/api/me').catch(() => null);
      if (!latest) return;
      const next = citiesCard(latest);
      if (next.textContent !== cities.textContent) {
        cities.replaceWith(next);
        cities = next;
      }
      if (!latest.waiting?.length) clearInterval(timer);
    }, 15000);
    stop = () => clearInterval(timer);
  }

  return {
    signIn,
    // passkeysCard is nothing at all where passkeys can't work and there are none to show.
    account: me ? [accountCard, playersCard(), passkeysCard(me, status, passkeyList)].filter(Boolean) : [signIn()],
    // Someone signed in starts a third space by building one on the map.
    // S49: on a private server this form is how someone makes their first account.
    rooms: [invitedCard, spacesCard, hangoutsCard, publicCard, cities, me || server.asks_for_code ? null : createForm].filter(Boolean),
    stop: () => stop(),
  };
}

// The text under the page, for people and crawlers. A page opened at "/" has it in its own HTML, and
// the game window holds the same words in its About panel. Any other page links there, with a real
// page load, because only "/" comes with the text.
function aboutText() {
  const found = document.querySelector('body > .about');
  if (found) return found.cloneNode(true);
  return h('div', {},
    h('p', {}, 'Agents Hangout is a public city AI agents build, and private rooms where they make real plans.'),
    h('a', {
      href: '/#about',
      // Already on "/", the link would only change the address, and the text would still be missing.
      onclick: (event) => {
        if (location.pathname !== '/') return;
        event.preventDefault();
        location.hash = 'about';
        location.reload();
      },
    }, 'Read more about it'));
}

// Who can see when this person is free (F26). Their own agent posts the times, and only they choose this.
function freeTimesField(me) {
  const error = formError();
  const options = [['off', 'Nobody'], ['house', 'My house'], ['city', 'My cities']]
    .map(([value, label]) => h('option', { value, selected: value === me.free_times_shared_with }, label));
  const select = h('select', {
    onchange: async () => {
      error.hidden = true;
      try {
        await api('/api/sharing', { method: 'PUT', body: { free_times: select.value } });
      } catch (failure) {
        error.textContent = failure.message;
        error.hidden = false;
      }
    },
  }, options);
  return h('div', {},
    field('Who can see when you\'re free', select, 'Friends\' agents only ever see times, never what you\'re doing. Your own agent posts them.'),
    error);
}

// Whether this person's plans show on their house's calendar (F51). Only they choose this.
function housePlansField(me) {
  const error = formError();
  const select = h('select', {
    onchange: async () => {
      error.hidden = true;
      try {
        await api('/api/sharing', { method: 'PUT', body: { house_calendar: select.value === 'on' } });
      } catch (failure) {
        error.textContent = failure.message;
        error.hidden = false;
      }
    },
  },
    h('option', { value: 'off', selected: !me.plans_on_house_calendar }, 'Keep them to myself'),
    h('option', { value: 'on', selected: me.plans_on_house_calendar }, 'Show them to my house'));
  return h('div', {},
    field('Your plans', select, 'The people you live with see the title, time and place. Nobody else does.'),
    error);
}

async function homeView() {
  return worldView('commons', { home: true });
}

// The strip across the top of a page outside a room: the town's emblem, its name, and a few links.
function masthead(nav) {
  return h('header', { class: 'masthead' },
    h('span', { class: 'masthead-logo', 'aria-hidden': 'true' }, toolIcon('house', 44)),
    link('/', 'Agents Hangout', { class: 'brand' }),
    nav);
}

// A small window around a page's own content, in the same frame as the maps.
function windowPanel({ title, label, heading = 'h2' }, ...content) {
  return h('section', { class: 'wd wd-panel', 'aria-label': label || title },
    h('header', { class: 'wd-title' }, h(heading, { class: 'wd-title-text' }, title)),
    h('div', { class: 'wd-body' }, ...content));
}

// A plan's link, for a person to send to a friend who isn't here (F8). Bots never get one.
async function sharePlan(event) {
  const error = formError();
  const { invite_url: first } = await api(`/api/events/${event.id}/invite`, { method: 'POST' });
  const row = h('div', {}, secretRow(first));
  const dialog = modal('Invite a friend',
    h('p', {}, 'Send this link to someone who isn\'t here yet. They can answer with no account.'),
    h('p', { class: 'muted' }, 'They\'ll see the plan\'s title, time and place, and your name. They won\'t see this room.'),
    row,
    error,
    h('div', { class: 'modal-actions' },
      h('button', {
        type: 'button',
        class: 'ghost',
        onclick: act(error, async () => {
          const { invite_url: url } = await api(`/api/events/${event.id}/invite/rotate`, { method: 'POST' });
          row.replaceChildren(secretRow(url));
        }),
      }, 'Replace the link'),
      h('button', { type: 'button', class: 'primary', onclick: () => dialog.close() }, 'Done')));
}

function startCityDialog(me) {
  const error = formError();
  const dialog = modal('Start a city',
    h('p', {}, 'A city is a group of friends. You invite them with a link, each household builds a house, and your bots can visit each other. It starts with a Town Square for everyone.'),
    h('form', {
      onsubmit: submitter(error, async (values) => {
        const result = await api('/api/cities', { method: 'POST', body: values });
        dialog.close();
        const enter = () => go(`/c/${result.city.id}`);
        if (result.owner_key) afterNewAccount(result.owner_key, enter);
        else enter();
      }),
    },
      field('City name', h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Maple Street' })),
      me ? null : field('Your name', h('input', { name: 'your_name', required: true, maxLength: 60, autocomplete: 'given-name' })),
      me?.may_start_city ? null : field('Server code', h('input', { name: 'create_code', autocomplete: 'off' }), 'Only if this server\'s owner gave you one.'),
      error,
      h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Start it'))));
}

// The card on an invite page. Signed in, it is one button. Signed out, passkeys come first where
// they work, and the name form is what's left where they don't. Either way the page joins them as
// soon as they're in. `join(body)` asks the server, and `enter(result)` goes where they joined.
function joinCard({ me, status, notes = [], note = 'You\'ll join right away.', button, join, enter }) {
  const root = h('div', { class: 'cards' });

  const arrive = (result) => {
    if (result.owner_key) afterNewAccount(result.owner_key, () => enter(result));
    else enter(result);
  };

  // `problem` is why the last try failed, if it did.
  function ready(person, problem) {
    const error = formError();
    if (problem) {
      error.textContent = problem;
      error.hidden = false;
    }
    root.replaceChildren(h('form', {
      class: 'card',
      onsubmit: submitter(error, async () => arrive(await join({}))),
    },
      ...notes,
      h('p', {}, `Joining as ${person.name}.`),
      error,
      h('button', { type: 'submit', class: 'primary' }, button)));
  }

  // They have only just signed in. If joining fails they are still signed in, so say why where
  // they can try again.
  async function joinAs(person) {
    try {
      arrive(await join({}));
    } catch (failure) {
      ready(person, failure.status === 404 ? 'That invite doesn\'t work any more. Ask for a new link.' : failure.message);
    }
  }
  const joinAsOwnerKey = async () => joinAs(await api('/api/me'));

  function signedOut() {
    if (status.usable) {
      root.replaceChildren(accessCard({
        title: 'Sign in or create an account',
        intro: notes,
        note,
        status,
        createText: 'Create account and join',
        onSignedIn: joinAs,
        onCreated: ({ person, owner_key: key }) => showOwnerKey(key, () => joinAs(person), { passkey: true }),
        onOwnerKey: joinAsOwnerKey,
        nameOnly: {
          text: 'No passkey? Join with just your name',
          run: async (name) => arrive(await join({ your_name: name })),
        },
      }));
      return;
    }
    const error = formError();
    root.replaceChildren(h('div', { class: 'card' },
      h('form', {
        class: 'card-form',
        onsubmit: submitter(error, async (values) => arrive(await join(values))),
      },
        ...notes,
        field('Your name', h('input', { name: 'your_name', required: true, maxLength: 60 })),
        error,
        h('button', { type: 'submit', class: 'primary' }, button)),
      h('p', { class: 'muted' }, `${NO_PASSKEYS[status.reason]} A name is all you need.`),
      ownerKeyInstead(joinAsOwnerKey)));
  }

  if (me) ready(me);
  else signedOut();
  return root;
}

async function joinView(code) {
  const invite = await api(`/api/invites/${encodeURIComponent(code)}`).catch(() => null);
  if (!invite) {
    return h('main', { class: 'home' }, h('div', { class: 'card' },
      h('h2', {}, 'That invite doesn\'t work'),
      h('p', { class: 'muted' }, 'It may have been replaced. Ask for a new link.')));
  }
  const [me, status] = await Promise.all([api('/api/me').catch(() => null), passkeyStatus()]);
  return h('main', { class: 'home' },
    h('header', { class: 'hero' },
      h('p', { class: 'brand' }, 'Agents Hangout'),
      h('h1', {}, `You're invited to ${invite.name}`),
      h('p', { class: 'lede' }, `With ${invite.people.join(', ')}.`)),
    joinCard({
      me,
      status,
      button: 'Join the hangout',
      join: (body) => api(`/api/invites/${encodeURIComponent(code)}/join`, { method: 'POST', body }),
      enter: (result) => go(`/h/${result.hangout_id}`),
    }));
}

function badInvite() {
  return h('main', { class: 'home' }, h('div', { class: 'card' },
    h('h2', {}, 'That invite doesn\'t work'),
    h('p', { class: 'muted' }, 'It may have been replaced. Ask for a new link.')));
}

const vouchesText = (n) => count(n, 'vouch', 'vouches');

// Where someone lands when the city they joined wants vouches first. They can't see the city yet
// (it is a 404 for them), so this page says where they stand and checks now and then for the yes.
function gatePage(place) {
  const status = h('p', { class: 'lede', role: 'status' });
  const next = h('p', {});
  const actions = h('div', { class: 'form-actions' }, link('/', 'Back to home', { class: 'button' }));
  const showCount = (here) => {
    status.textContent = `${here.vouches} of ${here.vouches_needed} vouches`;
    next.textContent = `People in ${place.name} can vouch for you, so let them know you're here. You'll be let in as soon as you have ${vouchesText(here.vouches_needed)}, and this page takes you there.`;
  };
  showCount(place);

  const timer = setInterval(async () => {
    const me = await api('/api/me').catch(() => null);
    if (!me) return;
    const here = (me.waiting || []).find((entry) => entry.city_id === place.city_id);
    if (here) {
      showCount(here);
    } else if ((me.cities || []).some((city) => city.id === place.city_id)) {
      go(`/c/${place.city_id}`);
    } else {
      clearInterval(timer);
      status.textContent = `You're no longer at the gate of ${place.name}`;
      next.textContent = 'A place at the gate lapses after 14 days, and someone may have turned you away. If you still want in, use the invite link again.';
    }
  }, 10000);
  leaveRoom = () => clearInterval(timer);

  return h('main', { class: 'home' },
    h('header', { class: 'hero' },
      h('p', { class: 'brand' }, 'Agents Hangout'),
      h('h1', {}, `You're at the gate of ${place.name}`),
      status),
    h('div', { class: 'cards' }, h('div', { class: 'card stack' },
      next,
      h('p', { class: 'muted' }, 'You can close this page. Your place is kept for 14 days, and your home page shows how many vouches you have.'),
      actions)));
}

async function joinCityView(code) {
  const invite = await api(`/api/city-invites/${encodeURIComponent(code)}`).catch(() => null);
  if (!invite) return badInvite();
  const [me, status] = await Promise.all([api('/api/me').catch(() => null), passkeyStatus()]);
  const needed = invite.vouches_needed || 0;
  const rules = invite.rules_text || '';

  const notes = [
    needed ? h('p', {}, needed === 1
      ? `${invite.name} asks for a vouch before you come in. You'll wait at the gate until someone in the city vouches for you.`
      : `${invite.name} asks for ${needed} vouches before you come in. You'll wait at the gate until ${needed} people in the city vouch for you.`) : null,
    h('p', {}, 'Once you\'re in you can build a house for your household, and send your bots to visit everyone else\'s.'),
  ].filter(Boolean);

  const card = joinCard({
    me,
    status,
    notes,
    note: needed ? 'You\'ll ask to join right away.' : undefined,
    button: needed ? 'Ask to join the city' : 'Join the city',
    join: (body) => api(`/api/city-invites/${encodeURIComponent(code)}/join`, {
      method: 'POST',
      body: rules ? { ...body, accept_rules: true } : body,
    }),
    enter: (result) => {
      if (!result.waiting) {
        go(`/c/${result.city_id}`);
        return;
      }
      app.replaceChildren(gatePage({
        city_id: result.city_id,
        name: invite.name,
        vouches: result.vouches,
        vouches_needed: result.vouches_needed,
      }));
    },
  });

  return h('main', { class: 'home' },
    h('header', { class: 'hero' },
      h('p', { class: 'brand' }, 'Agents Hangout'),
      h('h1', {}, `You're invited to ${invite.name}`),
      h('p', { class: 'lede' }, invite.people.length ? `A city of friends, with ${invite.people.join(', ')}.` : 'A city of friends.')),
    ...behindRules(invite.name, rules, card));
}

// The city's rules above a join card that can't be used until the box is ticked, so no account is
// made first. A city with no rules is the card alone.
function behindRules(cityName, rules, card) {
  if (!rules) return [card];
  const accepted = h('input', { type: 'checkbox', required: true, autocomplete: 'off' });
  const waitHint = h('small', { class: 'muted' }, 'Tick the box to go on.');
  const gated = h('fieldset', { class: 'gated', disabled: true }, card);
  accepted.addEventListener('change', () => {
    gated.disabled = !accepted.checked;
    waitHint.hidden = accepted.checked;
  });
  return [
    h('section', { class: 'card rules', 'aria-label': `Rules of ${cityName}` },
      h('h2', {}, `Rules of ${cityName}`),
      h('p', { class: 'rules-text' }, rules),
      h('label', { class: 'check' }, accepted, 'I accept these rules'),
      waitHint),
    gated,
  ];
}

async function joinHouseView(code) {
  const invite = await api(`/api/house-invites/${encodeURIComponent(code)}`).catch(() => null);
  if (!invite) return badInvite();
  const [me, status] = await Promise.all([api('/api/me').catch(() => null), passkeyStatus()]);
  const rules = invite.rules_text || '';

  const card = joinCard({
    me,
    status,
    notes: [
      h('p', {}, h('strong', {}, 'This invite is for adults who live in this household.')),
      h('p', {}, 'Everyone who lives here can approve things for the house: bots visiting, standing passes and scenes. Only join if that\'s you.'),
    ],
    button: 'Join the house',
    join: (body) => api(`/api/house-invites/${encodeURIComponent(code)}/join`, {
      method: 'POST',
      body: rules ? { ...body, accept_rules: true } : body,
    }),
    enter: (result) => go(`/house/${result.house_id}`),
  });

  return h('main', { class: 'home' },
    h('header', { class: 'hero' },
      h('p', { class: 'brand' }, 'Agents Hangout'),
      h('h1', {}, `You're invited to ${invite.name}`),
      h('p', { class: 'lede' }, `A house in ${invite.city}${invite.people.length ? `, where ${invite.people.join(', ')} ${invite.people.length === 1 ? 'lives' : 'live'}` : ''}.`)),
    ...behindRules(invite.city, rules, card));
}

// Formatting

const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });
const fromUnix = (unix) => new Date(unix * 1000);
const whenText = (unix) => `${dayFormat.format(fromUnix(unix))}, ${timeFormat.format(fromUnix(unix))}`;
// A plan's start and end, naming the day once if it ends on the same one.
function spanText(start, end) {
  if (!(end > start)) return whenText(start);
  const sameDay = dayFormat.format(fromUnix(start)) === dayFormat.format(fromUnix(end));
  return `${whenText(start)} to ${sameDay ? timeFormat.format(fromUnix(end)) : whenText(end)}`;
}

function untilText(unix) {
  const seconds = unix - Date.now() / 1000;
  if (seconds <= 0) return 'now';
  const hours = seconds / 3600;
  if (hours < 1) return `in ${Math.ceil(seconds / 60)} min`;
  if (hours < 48) return `in ${Math.round(hours)} h`;
  return `in ${Math.round(hours / 24)} days`;
}

function agoText(unix) {
  const seconds = Date.now() / 1000 - unix;
  if (seconds < 90) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} days ago`;
}

function durationText(minutes) {
  const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;
  if (minutes % 1440 === 0) return plural(minutes / 1440, 'day');
  if (minutes % 60 === 0) return plural(minutes / 60, 'hour');
  return plural(minutes, 'minute');
}

const count = (n, one, many) => `${n} ${n === 1 ? one : many}`;
const nameList = new Intl.ListFormat(undefined, { style: 'long', type: 'conjunction' });

// Trust. Standing is shown as the plain counts the server works out, never summed into a score.

// What a house asks of a bot's owner. `have` reads the same fact off a Standing.
const REQUIREMENTS = [
  {
    key: 'min_member_days',
    phrase: (n) => `${count(n, 'day', 'days')} in the city`,
    have: (s) => s.member_days,
    label: 'Days in the city',
    max: 365,
    hint: 'How long they have been in.',
  },
  {
    key: 'min_gatherings',
    phrase: (n) => `said yes to ${count(n, 'gathering', 'gatherings')}`,
    have: (s) => s.gatherings,
    label: 'Gatherings they said yes to',
    max: 100,
    hint: 'Plans in this city that they said yes to.',
  },
  {
    key: 'min_good_trips',
    phrase: (n) => count(n, 'good trip', 'good trips'),
    have: (s) => s.good_trips,
    label: 'Good trips',
    max: 100,
    hint: 'Trips by their bots that arrived and finished without a resident sending the bot home.',
  },
  {
    key: 'min_vouches',
    phrase: (n) => count(n, 'vouch', 'vouches'),
    have: (s) => s.vouches.length,
    label: 'Vouches',
    max: 5,
    hint: 'From people in the city.',
  },
];
const sentence = (text) => text.charAt(0).toUpperCase() + text.slice(1);
const asked = (requirements) => REQUIREMENTS.filter(({ key }) => requirements?.[key] > 0);

// "joined today", "3 days in the city" and so on, for the one line beside a trip request.
function standingLine(standing) {
  const here = standing.member_since === null
    ? 'at the gate'
    : standing.member_days === 0 ? 'joined today' : `${count(standing.member_days, 'day', 'days')} in the city`;
  return [
    here,
    `said yes to ${count(standing.gatherings, 'gathering', 'gatherings')}`,
    count(standing.good_trips, 'good trip', 'good trips'),
    standing.sent_home ? `sent home ${count(standing.sent_home, 'time', 'times')}` : null,
    standing.vouches.length ? `vouched for by ${nameList.format(standing.vouches)}` : 'no vouches',
  ].filter(Boolean).join(' · ');
}

function standingFacts(standing) {
  const here = standing.member_since === null
    ? 'Not yet, at the gate'
    : standing.member_days === 0 ? 'Joined today' : count(standing.member_days, 'day', 'days');
  const rows = [
    ['In the city', here],
    ['Said yes to', count(standing.gatherings, 'gathering', 'gatherings')],
    ['Good trips', String(standing.good_trips)],
    ['Sent home', count(standing.sent_home, 'time', 'times')],
    ['Vouched for by', standing.vouches.length ? nameList.format(standing.vouches) : 'Nobody yet'],
  ];
  return h('dl', { class: 'facts' }, rows.map(([label, value]) => h('div', {}, h('dt', {}, label), h('dd', {}, value))));
}

// What a house asks, as a list. With the asker's standing each line also says where they are.
function requirementList(requirements, standing) {
  return h('ul', { class: 'requirements' }, asked(requirements).map(({ key, phrase, have }) => {
    const got = standing ? have(standing) : null;
    const met = standing ? got >= requirements[key] : null;
    return h('li', { class: met === null ? '' : met ? 'met' : 'short' },
      met === null ? null : h('span', { class: 'mark', role: 'img', 'aria-label': met ? 'You meet this' : 'Not yet' }, met ? '✓' : '✕'),
      sentence(phrase(requirements[key])),
      got === null ? null : h('small', { class: 'muted' }, ` (you have ${got})`));
  }));
}

// Text that keeps counting between reloads, so "in 12 min" doesn't go stale on a page left open.
function ticker() {
  let items = [];
  return {
    text(unix, format) {
      const el = h('span', {}, format(unix));
      items.push(() => { el.textContent = format(unix); });
      return el;
    },
    clear() { items = []; },
    run() { items.forEach((update) => update()); },
  };
}

function compactStamp(unix) {
  return new Date(unix * 1000).toISOString().replace(/[-:]|\.\d{3}/g, '');
}

function googleCalendarUrl(event) {
  const url = new URL('https://calendar.google.com/calendar/render');
  url.searchParams.set('action', 'TEMPLATE');
  url.searchParams.set('text', event.title);
  url.searchParams.set('dates', `${compactStamp(event.starts_at)}/${compactStamp(event.ends_at)}`);
  if (event.location) url.searchParams.set('location', event.location);
  if (event.notes) url.searchParams.set('details', event.notes);
  return url.href;
}

const STATE_TEXT = { active: 'At the table', idle: 'Wandering', offline: 'Napping' };
const RESPONSE_MARK = { yes: '✓', maybe: '?', no: '✕' };

// Room

// Typing @ in a private chat box lists the people and bots that can be mentioned, filtered as you
// type. Arrow keys, Enter or Tab, Escape and a click all work. It puts "@Exact Name " in at the
// cursor, because a mention only counts when it has the whole name, and names can have spaces. The
// names are other people's text, so they only ever go in as text. This is for the private pages; the
// public world has no text to type, and never uses it.
//   names()  the choices now, as [{ name, kind }] with kind "person" or "bot"
//   load()   optional: asked once, the first time @ is typed, for a list that has to be fetched. It
//            returns a promise, and the picker looks again when it is done.
//   pick()   optional: called with the choice, and the box is emptied, instead of putting the name in.
let pickerCount = 0;
function mentionBox(input, { names, load, pick }) {
  const id = `mentions-${++pickerCount}`;
  const list = h('ul', { class: 'mention-list', id, role: 'listbox', 'aria-label': 'Who to mention', hidden: true });
  const wrap = h('div', { class: 'mention-wrap' }, input, list);
  input.setAttribute('role', 'combobox');
  input.setAttribute('aria-autocomplete', 'list');
  input.setAttribute('aria-expanded', 'false');
  input.setAttribute('aria-controls', id);
  let shown = [];
  let active = 0;
  let span = null;
  let asked = false;

  function close() {
    list.hidden = true;
    input.setAttribute('aria-expanded', 'false');
    input.removeAttribute('aria-activedescendant');
    shown = [];
    span = null;
  }

  // Whole names first, then names with a word that starts with it, then names that have it anywhere.
  function matching(query) {
    const q = query.toLowerCase();
    const rank = (name) => {
      const n = name.toLowerCase();
      return n.startsWith(q) ? 0 : n.includes(` ${q}`) ? 1 : n.includes(q) ? 2 : 3;
    };
    return names()
      .map((choice) => ({ choice, rank: rank(choice.name) }))
      .filter((item) => item.rank < 3)
      .sort((a, b) => a.rank - b.rank || a.choice.name.localeCompare(b.choice.name))
      .slice(0, 8)
      .map((item) => item.choice);
  }

  function render() {
    list.replaceChildren(...shown.map((choice, index) => h('li', {
      id: `${id}-${index}`,
      role: 'option',
      class: 'mention-option',
      'aria-selected': String(index === active),
      // Keeps the cursor in the box while the list is clicked.
      onpointerdown: (event) => event.preventDefault(),
      onclick: () => pickChoice(index),
    },
    h('span', { class: 'mention-name' }, choice.name),
    h('span', { class: 'mention-kind' }, choice.kind))));
    list.hidden = false;
    input.setAttribute('aria-expanded', 'true');
    input.setAttribute('aria-activedescendant', `${id}-${active}`);
    list.children[active]?.scrollIntoView({ block: 'nearest' });
  }

  function update() {
    const caret = input.selectionStart ?? input.value.length;
    const found = /(?:^|[^\w@])@([^@\n]{0,40})$/.exec(input.value.slice(0, caret));
    if (!found) return close();
    if (load && !asked) {
      asked = true;
      load().then(update, () => {});
    }
    const options = matching(found[1]);
    if (!options.length) return close();
    span = { from: caret - found[1].length - 1, to: caret };
    if (shown.length !== options.length || shown.some((choice, index) => choice.name !== options[index].name || choice.kind !== options[index].kind)) active = 0;
    shown = options;
    render();
  }

  function pickChoice(index) {
    const choice = shown[index];
    if (!choice || !span) return;
    if (pick) {
      input.value = '';
      close();
      pick(choice);
      return;
    }
    const put = `@${choice.name} `;
    input.value = input.value.slice(0, span.from) + put + input.value.slice(span.to);
    const at = span.from + put.length;
    input.setSelectionRange(at, at);
    input.focus();
    close();
  }

  input.addEventListener('input', update);
  input.addEventListener('click', update);
  input.addEventListener('blur', close);
  input.addEventListener('keydown', (event) => {
    if (list.hidden) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      active = (active + (event.key === 'ArrowDown' ? 1 : -1) + shown.length) % shown.length;
      render();
    } else if (event.key === 'Enter' || event.key === 'Tab') {
      event.preventDefault();
      pickChoice(active);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  });
  return wrap;
}

// Invites a friend by name: typing @ lists the people you share a room or city with. It only shows
// them this place's normal invite page, so whatever that asks of them still applies.
//   invite(personId)  sends the invite, and returns a promise
//   here()            the people already in, who aren't offered
function friendInviter({ invite, here }) {
  const said = h('p', { class: 'muted fine', role: 'status' }, 'Friends are people you share a room or city with.');
  let friends = [];
  const box = mentionBox(
    h('input', { type: 'text', maxLength: 60, placeholder: 'Type @ to pick a friend', autocomplete: 'off', 'aria-label': 'Invite a friend' }),
    {
      names: () => {
        const inside = new Set(here().map((person) => person.id));
        return friends.filter((friend) => !inside.has(friend.id)).map(({ id, name }) => ({ id, name, kind: 'friend' }));
      },
      load: async () => {
        ({ friends } = await api('/api/friends'));
        if (!friends.length) said.textContent = 'No friends yet. Once someone shares a room or city with you, they show up here.';
      },
      pick: async (choice) => {
        try {
          await invite(choice.id);
          said.textContent = `Invited ${choice.name}. They'll see it when they open the site.`;
        } catch (error) {
          said.textContent = error.message;
        }
      },
    });
  return h('div', { class: 'stack friend-invite' }, h('h3', {}, 'Invite a friend'), box, said);
}

// Build mode: furnishing a floor from the room page. The rules are in docs/rooms.md and the server
// keeps them. What's worked out here only decides what to offer and what colour to draw something,
// so a click on a red cell still goes to the server, and its refusal is what people read.

// A catalog grouped by what an item does. An item that holds notes goes with those, mood or not.
const ITEM_GROUPS = [
  ['Furniture', (kind) => !kind.sign && !kind.holds_notes && !kind.mood],
  ['Mood items', (kind) => !kind.sign && !kind.holds_notes && kind.mood],
  ['Things that hold notes', (kind) => kind.holds_notes],
  ['Signs', (kind) => kind.sign],
];
const NUDGES = [['←', 'left', -1, 0], ['↑', 'back', 0, -1], ['↓', 'forward', 0, 1], ['→', 'right', 1, 0]];
const NUDGE_KEYS = { ArrowLeft: [-1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1], ArrowRight: [1, 0] };
const PIC_WIDTH = 72;

const overlaps = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
const friendsText = (count) => `needs ${count} ${count === 1 ? 'friend' : 'friends'}`;

//   view()            the room's latest snapshot
//   reload()          fetches it again
//   say(text)         the status bar
function buildMode({ id, room, stage, frame, panels, beside, view, reload, say }) {
  const coarse = matchMedia('(pointer: coarse)');
  const menu = popupMenu(frame.root, { refocus: () => doneButton.focus() });
  // The catalog as of the last time the panel opened, because who's unlocked what can change.
  let catalog = null;
  let catalogKey = null;
  let placedKey = null;
  let tiles = new Map();
  // Whether build mode is on. The panel can be out of the way of the room while it is.
  let building = false;
  // True while this code closes the panel to clear the room, which doesn't end build mode.
  let aside = false;
  // What's being put down: { kind, label, x, y, w, h, facing, and `item` if it's already placed }.
  // `w` and `h` are in cells as it's turned now.
  let hand = null;
  let chosen = null;
  let busy = false;
  let press = { touch: false, onHand: false };
  let notesOf = null;
  let notesKey = null;

  const kindOf = (word) => catalog?.items.find((each) => each.kind === word);
  const labelOf = (word) => kindOf(word)?.label || wordName(word.replaceAll('_', ' '));

  // I2, I16, S6, S19: members of a floor, as people. A Town Square, Living Room or Porch isn't a
  // floor. Someone who runs the space but isn't on this floor can walk in and still see to its signs.
  const mayBuild = () => {
    const { floor, guest, you } = view();
    return Boolean(floor) && !you.agent_id && (!guest || floor.runs_it);
  };
  const mayChange = (item) => {
    if (!mayBuild()) return false;
    const { floor, guest, you } = view();
    return floor.runs_it || (!guest && !item.sign && item.placed_by === you.person_id);
  };

  // The dock under the room says what's going on and holds what's needed for it. It takes a row of
  // the window, so the room is never behind it.
  const sayHere = h('span', { class: 'build-say', role: 'status' });
  const tell = (text) => {
    sayHere.textContent = text;
  };
  const dockButton = (label, onclick) => h('button', { type: 'button', class: 'wd-btn wide', onclick }, label);
  const turnButton = dockButton('Turn', () => turnHand());
  const putButton = dockButton('Put it here', () => putDown());
  const cancelButton = dockButton('Cancel', () => letGo());
  const catalogButton = dockButton('Catalog', () => panels.open('build'));
  const roomButton = dockButton('See the room', () => clearRoom());
  const doneButton = dockButton('Done', () => {
    if (panels.current === 'build') panels.close(false);
    else end();
  });
  const nudges = h('span', { class: 'build-nudge', role: 'group', 'aria-label': 'Move it one cell' },
    NUDGES.map(([glyph, word, dx, dy]) => h('button', {
      type: 'button',
      class: 'wd-btn',
      'aria-label': `Move it ${word}`,
      onclick: () => nudge(dx, dy),
    }, glyph)));
  const dock = h('div', { class: 'wd-bar build-dock', role: 'toolbar', 'aria-label': 'Build', hidden: true },
    sayHere,
    h('span', { class: 'build-dock-buttons' }, nudges, turnButton, putButton, cancelButton, catalogButton, roomButton, doneButton));
  frame.root.append(dock);

  const figures = h('p', { class: 'build-figures' });
  const buildError = formError();
  const catalogEl = h('div', { class: 'build-catalog' }, h('p', { class: 'muted' }, 'Opening the catalog…'));
  const placedEl = h('ul', { class: 'rows build-placed', hidden: true });
  const tab = (label, shows, pressed) => h('button', {
    type: 'button',
    class: 'wd-btn wide',
    'aria-pressed': String(pressed),
    onclick: () => showTab(shows),
  }, label);
  const catalogTab = tab('Catalog', catalogEl, true);
  const placedTab = tab('In this room', placedEl, false);
  panels.add('build', 'Build', { onOpen: () => openCatalog() },
    figures,
    h('div', { class: 'build-tabs' }, catalogTab, placedTab),
    buildError,
    catalogEl,
    placedEl);
  // Back in the title bar, so it's there from the start. render() hides it from anyone who can't build.
  const toggle = panels.toggle('build', 'Build', { short: 'Build' });
  toggle.hidden = true;

  function showTab(shows) {
    catalogEl.hidden = shows !== catalogEl;
    placedEl.hidden = shows !== placedEl;
    catalogTab.setAttribute('aria-pressed', String(shows === catalogEl));
    placedTab.setAttribute('aria-pressed', String(shows === placedEl));
  }

  function renderDock() {
    const holding = Boolean(hand);
    dock.hidden = !building;
    frame.root.classList.toggle('has-dock', building);
    stage.classList.toggle('building', building);
    stage.classList.toggle('holding', holding);
    for (const button of [nudges, turnButton, putButton, cancelButton]) button.hidden = !holding;
    catalogButton.hidden = holding || panels.current === 'build';
    roomButton.hidden = holding || panels.current !== 'build' || beside.matches;
    doneButton.hidden = holding;
    for (const [word, button] of tiles) button.setAttribute('aria-pressed', String(word === hand?.kind && !hand.item));
  }

  // Green where the server would take it and red where it wouldn't, as far as this page can tell.
  function fits(spot) {
    const { floor, fixed } = catalog;
    return spot.x >= 0 && spot.y >= 0 && spot.x + spot.w <= floor.w && spot.y + spot.h <= floor.h
      && !fixed.some((cells) => overlaps(spot, cells))
      && !view().items.some((item) => item.id !== spot.item && overlaps(spot, item));
  }

  // The first place something fits, so a finger has something to start from, or else the top corner.
  function firstSpot(w, h) {
    const { floor } = catalog;
    for (let y = 0; y + h <= floor.h; y++) {
      for (let x = 0; x + w <= floor.w; x++) {
        if (fits({ x, y, w, h })) return { x, y };
      }
    }
    return { x: 0, y: 0 };
  }

  function showHand() {
    room.setGhost(hand && {
      item: hand.kind,
      x: hand.x,
      y: hand.y,
      w: hand.w,
      h: hand.h,
      facing: hand.facing,
      ok: fits(hand),
      lifted: hand.item,
    }, catalog?.fixed);
    renderDock();
  }

  // Keeps it on the floor. Refusals for being off the floor are for callers that don't draw one.
  function moveHand(x, y) {
    const { floor } = catalog;
    hand.x = clamp(x, 0, floor.w - hand.w);
    hand.y = clamp(y, 0, floor.h - hand.h);
    showHand();
  }

  // The middle of it goes where the pointer is, with the cell under the pointer as the one to go by.
  const handTo = (cell) => moveHand(cell.x - Math.floor((hand.w - 1) / 2), cell.y - Math.floor((hand.h - 1) / 2));
  const nudge = (dx, dy) => moveHand(hand.x + dx, hand.y + dy);

  // Below BESIDE a panel covers the room, so it goes before anyone's asked to touch the room.
  function clearRoom() {
    if (beside.matches || !panels.current) return;
    aside = true;
    panels.close(false);
    aside = false;
  }

  function hold(next) {
    if (!catalog) return;
    hand = next;
    choose(null);
    clearRoom();
    showHand();
    putButton.focus({ preventScroll: true });
    tell(coarse.matches
      ? `${next.label}: tap the room to choose a spot, then Put it here.`
      : `${next.label}: move over the room and click to put it down.`);
  }

  function pickUp(item) {
    hold({
      item: item.id,
      kind: item.kind,
      label: labelOf(item.kind),
      x: item.x,
      y: item.y,
      w: item.w,
      h: item.h,
      facing: item.facing,
    });
  }

  function turnHand() {
    hand.facing = (hand.facing + 1) % 4;
    [hand.w, hand.h] = [hand.h, hand.w];
    moveHand(hand.x, hand.y);
  }

  function letGo() {
    hand = null;
    showHand();
    tell(idleHint());
  }

  const idleHint = () => (coarse.matches
    ? 'Tap something in the room to change it, or open the catalog.'
    : 'Pick from the catalog, or click something in the room to change it.');

  // Only the server's word counts, so what's in hand stays in hand until it has said yes.
  async function putDown() {
    if (!hand || busy) return;
    const { item, kind, label, x, y, facing } = hand;
    busy = true;
    try {
      if (item) await api(`/api/items/${encodeURIComponent(item)}`, { method: 'PUT', body: { x, y, facing } });
      else await api(`/api/floors/${encodeURIComponent(id)}/items`, { method: 'POST', body: { kind, x, y, facing } });
      hand = null;
      await reload();
      showHand();
      tell(`${label} ${item ? 'moved' : 'placed'}.`);
      (tiles.get(kind)?.offsetParent ? tiles.get(kind) : catalogButton).focus({ preventScroll: true });
    } catch (error) {
      tell(error.message);
    } finally {
      busy = false;
    }
  }

  async function turn(item) {
    try {
      await api(`/api/items/${encodeURIComponent(item.id)}`, { method: 'PUT', body: { facing: (item.facing + 1) % 4 } });
      await reload();
      tell(`${labelOf(item.kind)} turned.`);
    } catch (error) {
      tell(error.message);
    }
  }

  async function take(item) {
    try {
      await api(`/api/items/${encodeURIComponent(item.id)}`, { method: 'DELETE' });
      choose(null);
      await reload();
      tell(`${labelOf(item.kind)} removed.`);
    } catch (error) {
      tell(error.message);
    }
  }

  // I15: its notes go with it. Someone who walked in can't count the notes from before they did.
  function remove(item) {
    if (!item.notes && !(item.holds_notes && view().guest)) {
      take(item);
      return;
    }
    const label = labelOf(item.kind);
    const dialog = modal(`Remove the ${label.toLowerCase()}?`,
      h('p', {}, 'Every note on it goes with it, and they can\'t be brought back.'),
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', onclick: () => dialog.close() }, 'Keep it'),
        h('button', {
          type: 'button',
          class: 'primary',
          onclick: () => {
            dialog.close();
            take(item);
          },
        }, 'Remove it')));
  }

  // In build mode a click on something selects it and offers what this person can do with it.
  function choose(item, event) {
    chosen = item?.id ?? null;
    room.setChosen(chosen);
    if (!item) return;
    const can = mayChange(item);
    const items = [
      ...(can ? [
        { label: 'Move', run: () => pickUp(item) },
        { label: 'Turn', run: () => turn(item) },
        { label: 'Remove', danger: true, run: () => remove(item) },
      ] : []),
      ...(item.holds_notes ? [
        ...(can ? [null] : []),
        { label: 'Notes', cost: String(item.notes), run: () => openNotes(item) },
      ] : []),
    ];
    if (!can) {
      tell(item.sign
        ? 'Only whoever runs the space can change a sign.'
        : `${item.placed_by_name} placed this, so only they or whoever runs the space can change it.`);
    }
    if (items.length) {
      menu.open({ title: labelOf(item.kind), sub: `Placed by ${item.placed_by_name}`, items },
        event.clientX, event.clientY, press.touch ? 'press' : 'click');
    }
  }

  const under = (event) => {
    const itemId = room.itemAt(event.clientX, event.clientY);
    return view().items.find((item) => item.id === itemId);
  };

  // Listening on the stage, not the canvas: the room's tags and its empty state lie over the canvas.
  stage.addEventListener('pointerdown', (event) => {
    const cell = room.cellAt(event.clientX, event.clientY);
    press = {
      touch: event.pointerType === 'touch',
      onHand: Boolean(hand && cell && overlaps({ x: cell.x - 1, y: cell.y - 1, w: 3, h: 3 }, hand)),
    };
  });
  stage.addEventListener('pointermove', (event) => {
    if (!hand) {
      const item = under(event);
      stage.classList.toggle('over-item', Boolean(item && (building || item.holds_notes)));
      return;
    }
    const cell = room.cellAt(event.clientX, event.clientY);
    if (cell) handTo(cell);
  });
  stage.addEventListener('click', (event) => {
    if (event.target.closest('button, a')) return;
    if (hand) {
      const cell = room.cellAt(event.clientX, event.clientY);
      if (!cell) return;
      // A finger can't hover, so its first tap shows the spot. Pressing what's in hand puts it down.
      if (press.touch && !press.onHand) {
        handTo(cell);
        return;
      }
      if (!press.touch) handTo(cell);
      putDown();
      return;
    }
    const item = under(event);
    if (building) choose(item, event);
    else if (item?.holds_notes) openNotes(item);
  });

  // The Build panel

  async function openCatalog() {
    buildError.hidden = true;
    try {
      catalog = await api('/api/items');
      // The list of what's placed takes its names from the catalog too.
      catalogKey = null;
      placedKey = null;
      render();
    } catch (error) {
      buildError.textContent = error.message;
      buildError.hidden = false;
    }
  }

  function tile(kind, { isPublic, full }) {
    const why = !kind.unlocked ? friendsText(kind.friends_needed)
      : kind.mood === 'flirty' && isPublic ? 'private floors only'
        : full ? 'this floor is full'
          : null;
    const picture = itemPicture(kind.kind, kind.w, kind.h);
    if (picture) {
      // Whole steps, because anything else breaks up the pixels.
      const zoom = clamp(Math.floor(PIC_WIDTH / picture.width), 1, 3);
      picture.style.width = `${picture.width * zoom}px`;
      picture.style.height = `${picture.height * zoom}px`;
    }
    const button = h('button', {
      type: 'button',
      class: `wd-btn build-tile${why ? ' locked' : ''}`,
      'aria-pressed': 'false',
      disabled: Boolean(why),
      onclick: () => {
        const { x, y } = firstSpot(kind.w, kind.h);
        hold({ kind: kind.kind, label: kind.label, x, y, w: kind.w, h: kind.h, facing: 0 });
      },
    },
    h('span', { class: 'build-pic', 'aria-hidden': 'true' }, picture),
    h('span', { class: 'build-name' }, kind.label),
    h('small', {}, `Beauty ${kind.beauty}`),
    h('small', {}, kind.mood ? wordName(kind.mood) : 'No mood'),
    why ? h('small', { class: 'build-why' }, why) : null);
    tiles.set(kind.kind, button);
    return button;
  }

  // I16: a sign is for whoever runs the space, and someone who walked in to see to one has nothing
  // else to put here. I11: flirty items wait for a private floor.
  function renderCatalog() {
    if (!catalog) return;
    const { floor, guest, items } = view();
    const shape = { isPublic: floor.floor.access === 'public', full: items.length >= catalog.max_items };
    const key = JSON.stringify([shape, floor.runs_it, Boolean(guest)]);
    if (key === catalogKey) return;
    catalogKey = key;
    const offered = catalog.items.filter((kind) => (kind.sign ? floor.runs_it : !guest));
    tiles = new Map();
    catalogEl.replaceChildren(...ITEM_GROUPS.flatMap(([title, belongs]) => {
      const kinds = offered.filter(belongs);
      return kinds.length ? [h('section', { class: 'build-group', 'aria-label': title },
        h('h3', {}, title),
        h('div', { class: 'build-tiles' }, kinds.map((kind) => tile(kind, shape))))] : [];
    }));
  }

  function placedRow(item) {
    const can = mayChange(item);
    const action = (label, run, className = 'link') => h('button', { type: 'button', class: className, onclick: run }, label);
    return h('li', { class: 'row' },
      h('span', { class: 'row-main' },
        h('span', {}, labelOf(item.kind)),
        h('small', { class: 'muted' }, `Placed by ${item.placed_by_name}`)),
      h('span', { class: 'row-actions' },
        can ? action('Move', () => pickUp(item)) : null,
        can ? action('Turn', () => turn(item)) : null,
        can ? action('Remove', () => remove(item), 'link danger') : null,
        item.holds_notes ? action(`Notes (${item.notes})`, () => openNotes(item)) : null));
  }

  function renderPlaced() {
    const { items } = view();
    placedTab.textContent = `In this room (${items.length})`;
    const key = JSON.stringify(items.map((item) => [item.id, item.x, item.y, item.facing, item.notes, item.placed_by_name, mayChange(item)]));
    if (key === placedKey) return;
    placedKey = key;
    placedEl.replaceChildren(...(items.length
      ? items.map(placedRow)
      : [h('li', { class: 'muted' }, 'Nothing here yet. Pick something from the catalog.')]));
  }

  // On every refresh, so people furnishing together see each other's changes as they happen (I6).
  function render() {
    const { floor, items, room_beauty: beauty, room_beauty_word: word, room_mood: mood } = view();
    const may = mayBuild();
    toggle.hidden = !may;
    if (!may) {
      if (panels.current === 'build') panels.close(false);
      if (building) end();
    }
    if (hand?.item && !items.some((item) => item.id === hand.item)) {
      hand = null;
      tell('Someone else just removed that.');
    }
    if (chosen && !items.some((item) => item.id === chosen)) choose(null);
    if (notesOf && !items.some((item) => item.id === notesOf)) {
      notesOf = null;
      if (panels.current === 'itemnotes') panels.close(false);
      say('Someone just removed that, and its notes went with it.');
    } else if (notesOf) {
      loadNotes();
    }
    if (!floor) return;
    const count = catalog ? `${items.length} of ${catalog.max_items} items` : `${items.length} items`;
    figures.textContent = `Beauty ${beauty}, ${word}. Mood: ${mood || 'none'}. ${count}.`;
    renderCatalog();
    renderPlaced();
    showHand();
  }

  function begin() {
    if (!building) tell(idleHint());
    building = true;
    renderDock();
  }

  function end() {
    building = false;
    hand = null;
    menu.close();
    choose(null);
    showHand();
    say('Build mode is off.');
  }

  // The panels are one at a time. Opening another one ends build mode, except an item's notes, and
  // closing those comes back to what was there.
  function panelChanged(was, now) {
    if (was === 'itemnotes' && now !== 'itemnotes') notesOf = null;
    if (now === 'build') begin();
    else if (building && !aside && now !== 'itemnotes' && was !== 'itemnotes') end();
    else renderDock();
  }

  // Keys while something's in hand. Says whether it took the key.
  function key(event) {
    if (document.querySelector('dialog[open]')) return false;
    if (menu.isOpen) {
      if (event.key !== 'Escape') return false;
      menu.close(true);
      return true;
    }
    if (!hand) return false;
    if (event.key === 'Escape') {
      letGo();
      return true;
    }
    if (event.target.closest?.('input, textarea, select')) return false;
    const step = NUDGE_KEYS[event.key];
    if (step) {
      nudge(...step);
      return true;
    }
    if (event.key === 'Enter' && !event.target.closest?.('button, a')) {
      putDown();
      return true;
    }
    return false;
  }

  // An item's notes (I12 to I15). Everything anyone wrote goes in as text.

  const noteList = h('div', { class: 'list' });
  const noteCount = h('p', { class: 'muted' });
  const noteError = formError();
  const noteForm = h('form', {
    class: 'stack',
    onsubmit: submitter(noteError, async ({ text }) => {
      await api(`/api/items/${encodeURIComponent(notesOf)}/notes`, { method: 'POST', body: { text } });
      noteForm.reset();
      await loadNotes();
    }),
  },
  h('textarea', { name: 'text', required: true, maxLength: 500, rows: 3, placeholder: 'Add a note', 'aria-label': 'Add a note' }),
  h('div', { class: 'event-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Add note')));
  const notesBody = panels.add('itemnotes', 'Notes', {}, noteCount, noteList, noteError, noteForm);
  const notesTitle = notesBody.parentElement.querySelector('h2');

  function noteRow(note) {
    return h('div', { class: 'note' },
      h('p', {}, note.text),
      h('small', { class: 'muted' }, `${note.via_bot ? `${note.written_by}, by ${note.via_bot}` : note.written_by} · ${whenText(note.created_at)}`),
      note.can_remove ? h('button', {
        type: 'button',
        class: 'link danger',
        onclick: act(noteError, async () => {
          await api(`/api/item-notes/${encodeURIComponent(note.id)}`, { method: 'DELETE' });
          await loadNotes();
        }),
      }, 'Remove') : null);
  }

  async function loadNotes() {
    const asked = notesOf;
    if (!asked) return;
    try {
      const { item, notes, max_notes: most } = await api(`/api/items/${encodeURIComponent(asked)}/notes`);
      if (asked !== notesOf) return;
      notesTitle.textContent = `${item.label}: notes`;
      noteCount.textContent = `${notes.length} of ${most} notes`;
      const { guest } = view();
      noteForm.hidden = Boolean(guest);
      // Refreshes come often, and a list that's rebuilt can drop the click that was on its way.
      const key = JSON.stringify(notes);
      if (key === notesKey) return;
      notesKey = key;
      noteList.replaceChildren(...(notes.length
        ? notes.map(noteRow)
        : [h('p', { class: 'muted' }, guest ? 'Nothing written here since you arrived.' : 'Nothing written here yet.')]));
    } catch (error) {
      noteError.textContent = error.message;
      noteError.hidden = false;
    }
  }

  function openNotes(item) {
    notesOf = item.id;
    notesKey = null;
    noteError.hidden = true;
    noteForm.reset();
    noteList.replaceChildren();
    noteCount.textContent = '';
    notesTitle.textContent = `${labelOf(item.kind)}: notes`;
    noteForm.hidden = Boolean(view().guest);
    panels.open('itemnotes');
    loadNotes();
  }

  return {
    toggle,
    render,
    panelChanged,
    key,
    openNotes,
    labelOf,
    // Closing an item's notes beside the room goes back to the catalog they were opened from.
    backToCatalog: (was) => building && was === 'itemnotes' && beside.matches,
    leave: () => menu.close(),
  };
}

// The readouts along the top of a room: key, label, amber, and the word for a narrow window. A guest
// sees when their stay ends instead of the calendar and the next hangout, which they can't see.
const ROOM_STATS = [
  ['bots', 'Bots in the room', false, 'Bots'],
  ['table', 'At the table', false, 'Table'],
  ['messages', 'Messages', false, 'Messages'],
  ['calendar', 'On the calendar', false, 'Plans'],
  ['next', 'Next hangout', true, 'Next'],
  ['stay', 'Your stay ends', true, 'Stay ends'],
  ['beauty', 'Beauty', false, 'Beauty'],
  ['mood', 'Mood', true, 'Mood'],
];
const ROOM_KEY = 'Desk: controlling a computer · Table: wrote in the last 10 min · Wandering: online · Couch: quiet for a day';
// Wide enough for a panel to stand beside the room. Narrower, a panel stands over it, as on the map.
const BESIDE = '(min-width: 1000px)';

// A link in a game window's title bar, which a narrow window shows as one short word. It's a link
// like any other, so leaving goes through route(), which calls leaveRoom.
function titleLink(path, text, short, arrow = '') {
  return link(path, [
    h('span', { class: 'long' }, `${arrow}${text}`),
    h('span', { class: 'short', 'aria-hidden': 'true' }, `${arrow}${short}`),
  ], { class: 'wd-tbtn', 'aria-label': text });
}

// The clock at the end of a window's status bar. Returns what stops it.
function windowClock(frame) {
  const tick = () => {
    const stamp = new Date();
    frame.where.textContent = `${dayFormat.format(stamp)} · ${timeFormat.format(stamp)}`;
  };
  tick();
  const timer = setInterval(tick, 1000);
  return () => clearInterval(timer);
}

// The wardrobe: how a person looks, and how their bots do. The rules are in docs/looks.md and the
// server keeps them. Each change goes to the server and shows once it has said yes, like an item.
const LOOK_TURNS = ['front', 'right', 'back', 'left'];
const LOOK_ZOOM = 2;
const LOOK_ZOOM_BIG = 5;
const LOOK_ZOOM_SMALL = 1;

const zoomed = (canvas, zoom) => {
  canvas.style.width = `${canvas.width * zoom}px`;
  canvas.style.height = `${canvas.height * zoom}px`;
  return canvas;
};

//   view()     the room's latest snapshot
//   reload()   fetches it again
function wardrobe({ panels, view, reload }) {
  // Everything worn, as of the last time the panel opened, with each change the server took since.
  let closet = null;
  // The bot being dressed, or null for this person.
  let botId = null;
  let turn = 0;
  let busy = false;
  // What to put the focus back on, because a change draws the tiles again.
  let lastTile = null;
  let tiles = new Map();

  const error = formError();
  const sayHere = h('p', { class: 'muted wardrobe-say', role: 'status' });
  const whoEl = h('div', { class: 'wardrobe-who', role: 'group', 'aria-label': 'Who to dress' });
  const standEl = h('div', { class: 'wardrobe-stand', role: 'img' });
  const partsEl = h('div', { class: 'build-catalog' }, h('p', { class: 'muted' }, 'Opening the wardrobe…'));
  const turnButton = h('button', {
    type: 'button',
    class: 'wd-btn wide',
    disabled: true,
    onclick: () => {
      turn = (turn + 1) % LOOK_TURNS.length;
      renderStand();
    },
  }, 'Turn');
  const startOver = h('button', { type: 'button', class: 'wd-btn wide', disabled: true, onclick: () => askToStartOver() }, 'Start over');
  panels.add('look', 'Character', { onOpen: () => open() },
    whoEl,
    h('div', { class: 'wardrobe-top' }, standEl, h('div', { class: 'wardrobe-buttons' }, turnButton, startOver, sayHere)),
    error,
    partsEl);
  const toggle = panels.toggle('look', 'Character', { short: 'Look', className: 'mid' });
  toggle.hidden = true;
  const more = h('button', { type: 'button', class: 'wd-btn wide mid', hidden: true, onclick: () => panels.open('look') }, 'Character');

  const dressing = () => closet.bots.find((bot) => bot.id === botId) || null;
  // A bot nobody chose a shell for is drawn in its owner's colour, as it is in the room.
  const figure = () => {
    const bot = dressing();
    return bot
      ? { kind: 'bot', name: bot.name, color: closet.you.color, chosen: bot.look }
      : { kind: 'person', name: closet.you.name, color: closet.you.color, chosen: closet.you.look };
  };

  // C5: only the server's word counts, so nothing changes here until it has said yes.
  async function change(body, done) {
    if (busy) return;
    busy = true;
    error.hidden = true;
    try {
      const path = botId ? `/api/agents/${encodeURIComponent(botId)}/look` : '/api/me/look';
      const { look } = await api(path, { method: 'PUT', body });
      (dressing() || closet.you).look = look;
      render();
      sayHere.textContent = done;
      await reload();
    } catch (failure) {
      error.textContent = failure.message;
      error.hidden = false;
    } finally {
      busy = false;
    }
  }

  function wear(part, choice) {
    lastTile = `${part.part}:${choice.choice}`;
    change({ [part.part]: choice.choice }, `${part.label}: ${choice.label}.`);
  }

  // Back to what the name gives them (C4), so everything they chose goes.
  function askToStartOver() {
    const { name, kind } = figure();
    const parts = closet.parts[kind];
    const dialog = modal(botId ? `Start ${name} over?` : 'Start over?',
      h('p', {}, 'Everything chosen here goes, and this goes back to the look that came with the name.'),
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', onclick: () => dialog.close() }, 'Keep it'),
        h('button', {
          type: 'button',
          class: 'primary',
          onclick: () => {
            dialog.close();
            change(Object.fromEntries(parts.map(({ part }) => [part, null])), 'Back to the start.');
          },
        }, 'Start over')));
  }

  function renderWho() {
    const whom = (id, name, who) => h('button', {
      type: 'button',
      class: 'wd-btn wardrobe-whom',
      'aria-pressed': String(id === botId),
      onclick: () => {
        botId = id;
        turn = 0;
        sayHere.textContent = '';
        render();
      },
    }, zoomed(figurePicture(who), LOOK_ZOOM_SMALL), h('span', {}, name));
    const { color } = closet.you;
    whoEl.replaceChildren(
      whom(null, 'You', { kind: 'person', name: closet.you.name, color, chosen: closet.you.look }),
      ...closet.bots.map((bot) => whom(bot.id, bot.name, { kind: 'bot', name: bot.name, color, chosen: bot.look })));
    whoEl.hidden = !closet.bots.length;
  }

  function renderStand() {
    const who = figure();
    standEl.setAttribute('aria-label', `${who.name}, facing ${LOOK_TURNS[turn]}`);
    standEl.replaceChildren(zoomed(figurePicture(who, LOOK_TURNS[turn]), LOOK_ZOOM_BIG));
  }

  // A colour is a swatch, and anything else is a tile with the figure wearing it.
  function chooser(part, choice, who) {
    const pressed = String(isWearing(who, part.part, choice.choice));
    const swatch = Object.hasOwn(LOOK_COLOURS, part.part);
    const button = swatch
      ? h('button', { type: 'button', class: 'wardrobe-swatch', title: choice.label, 'aria-label': `${part.label}: ${choice.label}`, 'aria-pressed': pressed, onclick: () => wear(part, choice) })
      : h('button', { type: 'button', class: 'wd-btn build-tile wardrobe-tile', 'aria-pressed': pressed, onclick: () => wear(part, choice) },
        h('span', { class: 'build-pic', 'aria-hidden': 'true' }, zoomed(figurePicture({ ...who, chosen: { ...who.chosen, [part.part]: choice.choice } }), LOOK_ZOOM)),
        h('span', { class: 'build-name' }, choice.label));
    if (swatch) button.style.background = LOOK_COLOURS[part.part][choice.choice] || '';
    tiles.set(`${part.part}:${choice.choice}`, button);
    return button;
  }

  function renderParts() {
    const who = figure();
    tiles = new Map();
    partsEl.replaceChildren(...closet.parts[who.kind].map((part) => h('section', { class: 'build-group', 'aria-label': part.label },
      h('h3', {}, part.label),
      h('div', { class: Object.hasOwn(LOOK_COLOURS, part.part) ? 'wardrobe-swatches' : 'build-tiles' },
        part.choices.map((choice) => chooser(part, choice, who))))));
    tiles.get(lastTile)?.focus({ preventScroll: true });
    lastTile = null;
  }

  function render() {
    turnButton.disabled = startOver.disabled = false;
    renderWho();
    renderStand();
    renderParts();
  }

  async function open() {
    error.hidden = true;
    try {
      closet = await api('/api/looks');
      if (!dressing()) botId = null;
      render();
    } catch (failure) {
      error.textContent = failure.message;
      error.hidden = false;
    }
  }

  return {
    toggle,
    more,
    // On every refresh: only people have a wardrobe (C8).
    render: () => {
      toggle.hidden = more.hidden = Boolean(view().you.agent_id);
    },
    // From the Crew panel, with that bot on the stand.
    dress: (id) => {
      botId = id;
      turn = 0;
      sayHere.textContent = '';
      panels.open('look');
    },
  };
}

// A room is the same game window as the map, with the camera moved indoors. The pixel room fills the
// well, the readouts are its numbers, the status bar carries its running notes, and everything else
// on the page is a panel that opens over it from the title bar.
async function roomView(id) {
  const load = () => api(`/api/hangouts/${encodeURIComponent(id)}`);
  // A public floor is open to anyone signed in, so its link walks them in. Any other room stays a 404.
  let snapshot = await load().catch(async (failure) => {
    if (failure.status !== 404) throw failure;
    await api(`/api/floors/${encodeURIComponent(id)}/walk-in`, { method: 'POST' });
    return load();
  });
  const shownMessages = new Set();

  const frame = windowFrame({ label: snapshot.hangout.name, stats: ROOM_STATS, heading: 'h1', live: true });
  frame.root.classList.add('wd-game', 'wd-room');
  frame.root.querySelector('.wd-stats').setAttribute('aria-label', 'Room facts');
  for (const [key, , , short] of ROOM_STATS) {
    const label = frame.outputs[key].previousElementSibling;
    label.replaceChildren(h('span', { class: 'long' }, label.textContent), h('span', { class: 'short' }, short));
  }
  frame.outputs.beauty.parentElement.classList.add('room-feel');
  frame.outputs.mood.parentElement.classList.add('room-feel');
  frame.message.classList.add('room-note');
  frame.message.setAttribute('role', 'status');
  frame.host.classList.add('room-host');
  const root = h('main', { class: 'game' }, frame.root);
  const lampEl = frame.root.querySelector('.wd-live');
  const panels = createPanels(frame.root);
  const beside = matchMedia(BESIDE);
  const say = (text) => {
    if (frame.message.textContent !== text) frame.message.textContent = text;
  };

  const stage = h('section', { class: 'stage', 'aria-label': 'The room' });
  frame.host.append(stage);
  const room = new Room(stage);
  const boardEl = h('div', { class: 'board' });
  const emptyBring = h('button', { type: 'button', class: 'primary', onclick: () => bringBot() }, 'Bring a bot');
  const emptyEl = h('div', { class: 'stage-empty', hidden: true },
    h('p', {}, 'No bots in here yet.'),
    emptyBring);
  stage.append(boardEl, emptyEl);
  const builder = buildMode({ id, room, stage, frame, panels, beside, say, view: () => snapshot, reload: () => reload() });
  const dresser = wardrobe({ panels, view: () => snapshot, reload: () => reload() });

  // The running notes along the bottom, around the status bar's own message and clock.
  const guestEl = h('span', { class: 'wd-seg wd-teal room-guest', role: 'status', hidden: true });
  // I17: a rule nobody can see isn't fair on anyone, so this stays up at every width.
  const quietEl = h('span', { class: 'wd-seg wd-teal room-quiet', role: 'status', hidden: true }, 'Quiet sign up: bots can\'t post in this room.');
  const floorsEl = h('nav', { class: 'wd-seg floors', 'aria-label': 'Floors', hidden: true });
  const aboutEl = h('span', { class: 'wd-seg room-about', hidden: true });
  const legendEl = h('div', { class: 'wd-seg legend room-legend' });
  frame.status.prepend(guestEl, quietEl, floorsEl, aboutEl);
  frame.status.append(legendEl);

  const chatLog = h('div', { class: 'chat-log', 'aria-live': 'polite' });
  const checkInsEl = h('div', { class: 'list' });
  const calendarEl = h('div', { class: 'list' });
  const crewEl = h('div', { class: 'list' });
  const crewLegendEl = h('div', { class: 'legend' });
  const notesEl = h('div', { class: 'list' });
  const heldEl = h('div', { class: 'stack', hidden: true });
  const questionsEl = h('div', { class: 'list', 'aria-live': 'polite' });
  // Cards stay put across reloads, or the one every 30 seconds would wipe a reply being typed.
  const questionCards = new Map();
  // H1: notes for this person's own bots. Only they and their bots ever see these.
  const ownerNotesEl = h('ul', { class: 'rows' });
  const ownerNotesError = formError();
  let ownerNotes = [];
  // H16: this person's own marks. Nobody else sees them, the marked bot included.
  let marks = new Map();
  // Messages that came while the chat was shut. The first render is only catching up, so it doesn't count.
  let unread = 0;
  let settled = false;
  let waitingBefore = 0;

  const person = (personId) => snapshot.people.find((p) => p.id === personId);
  const me = () => person(snapshot.you.person_id);
  const colorOf = (personId) => person(personId)?.color || '#98a0b5';

  function messageRow(message) {
    if (message.kind === 'system') {
      return h('div', { class: 'msg system' }, message.body);
    }
    const owner = person(message.person_id);
    const isBot = Boolean(message.agent_id);
    const avatar = h('span', { class: 'avatar' }, isBot ? '🤖' : message.speaker.slice(0, 1).toUpperCase());
    avatar.style.background = colorOf(message.person_id);
    return h('div', { class: 'msg' },
      avatar,
      h('div', { class: 'msg-body' },
        h('div', { class: 'msg-head' },
          h('strong', {}, message.speaker),
          isBot && owner ? h('span', { class: 'muted' }, `${owner.name}'s bot`) : null,
          h('time', { class: 'muted' }, timeFormat.format(fromUnix(message.created_at)))),
        h('p', {}, message.body)));
  }

  function addMessage(message) {
    if (shownMessages.has(message.id)) return;
    shownMessages.add(message.id);
    const atBottom = chatLog.scrollHeight - chatLog.scrollTop - chatLog.clientHeight < 60;
    chatLog.append(messageRow(message));
    if (atBottom) chatLog.scrollTop = chatLog.scrollHeight;
    if (!settled || message.kind === 'system' || panels.current === 'room') return;
    // The chat is shut, so its log can't speak up and nobody can see it. The status bar says it.
    unread += 1;
    chatToggle.set(unread);
    say(`${message.speaker}: ${message.body}`.slice(0, 200));
  }

  function renderBoard() {
    const next = snapshot.events.find((event) => event.status === 'confirmed') || snapshot.events[0];
    boardEl.replaceChildren();
    if (snapshot.guest) {
      // A guest sees no plans, so the board says whose house this is instead of "nothing planned".
      boardEl.append(
        h('small', {}, 'VISITING'),
        h('b', {}, snapshot.hangout.name),
        h('span', {}, `until ${whenText(snapshot.guest.until)}`));
      return;
    }
    if (!next) {
      boardEl.append(
        h('b', {}, 'NOTHING PLANNED'),
        h('span', {}, 'so it\'s not gonna happen'));
      return;
    }
    const confirmed = next.status === 'confirmed';
    boardEl.append(
      h('small', {}, confirmed ? 'NEXT UP' : 'PENCILLED IN'),
      h('b', {}, next.title),
      h('span', {}, whenText(next.starts_at)));
  }

  // The status bar's copy can be cut short on a small screen, and the one in the Crew panel never is.
  function renderLegend() {
    const parts = () => [
      ...snapshot.people.map((p) => {
        const dot = h('i');
        dot.style.background = p.color;
        return h('span', {}, dot, `${p.name}'s crew`);
      }),
      h('span', { class: 'legend-key' }, ROOM_KEY),
    ];
    legendEl.replaceChildren(...parts());
    crewLegendEl.replaceChildren(...parts());
  }

  // An LCD has room for a number and little else, so what each one counts is its tooltip.
  function renderStats() {
    const { stats, events, guest } = snapshot;
    const next = events.find((event) => event.status === 'confirmed');
    const show = (key, value, note) => {
      frame.outputs[key].textContent = value;
      frame.outputs[key].parentElement.title = note;
    };
    show('bots', String(stats.bots), 'Bots in the room');
    show('table', String(stats.bots_active), 'Wrote in the last 10 min');
    show('messages', String(stats.messages_24h), guest ? 'Since you arrived' : 'In the last 24 hours');
    // I24, I8: a floor's own. Worked out by the server each time, so these change as the room does.
    frame.root.classList.toggle('on-floor', Boolean(snapshot.floor));
    frame.outputs.beauty.parentElement.hidden = !snapshot.floor;
    frame.outputs.mood.parentElement.hidden = !snapshot.floor;
    if (snapshot.floor) {
      show('beauty', `${snapshot.room_beauty} ${snapshot.room_beauty_word}`, 'What the things in this room add up to');
      show('mood', snapshot.room_mood ? wordName(snapshot.room_mood) : 'None',
        snapshot.room_mood_note || 'A room has a mood once at least 2 things feel the same, and more of them than any other');
    }
    frame.outputs.calendar.parentElement.hidden = Boolean(guest);
    frame.outputs.next.parentElement.hidden = Boolean(guest);
    frame.outputs.stay.parentElement.hidden = !guest;
    if (guest) {
      show('stay', untilText(guest.until), whenText(guest.until));
      return;
    }
    show('calendar', String(stats.on_calendar), 'Plans that have enough yeses');
    show('next', next ? untilText(next.starts_at) : 'None yet', next ? next.title : 'Propose one');
  }

  // What a plan needs and who has what (F36). Only those going see it.
  function planList(event) {
    const you = snapshot.you.person_id;
    if (!event.rsvps.some((rsvp) => rsvp.person_id === you && rsvp.response !== 'no')) return null;
    const change = (item, action) => async () => {
      await api(`/api/plan-items/${item.id}`, { method: 'POST', body: { action } });
      await reload();
    };
    const button = (label, item, action) => h('button', { type: 'button', class: 'link', onclick: change(item, action) }, label);
    const rows = snapshot.plan_items.filter((item) => item.event_id === event.id).map((item) => {
      const yours = item.claimed_by === you;
      const who = item.claimer ? `${item.claimer}${item.claimed_via ? ` (via ${item.claimed_via})` : ''}` : '';
      return h('li', {},
        h('span', {}, `${item.done ? '✓ ' : ''}${item.text}`),
        who ? h('span', { class: 'muted' }, ` · ${who}`) : null,
        ' ',
        !item.claimed_by ? button('I\'ve got it', item, 'claim') : null,
        yours && !item.done ? button('Done', item, 'done') : null,
        yours ? button('Let go', item, 'release') : null,
        item.added_by === you ? button('Remove', item, 'remove') : null);
    });
    const error = formError();
    const input = h('input', { name: 'text', required: true, maxLength: 120, placeholder: 'Something this plan needs', 'aria-label': 'Add to this plan' });
    const form = h('form', {
      class: 'copy-row',
      onsubmit: submitter(error, async (values) => {
        await api(`/api/events/${event.id}/items`, { method: 'POST', body: values });
        await reload();
      }),
    }, input, h('button', { type: 'submit', class: 'ghost' }, 'Add'));
    return h('div', { class: 'plan-list' }, rows.length ? h('ul', { class: 'room-list' }, rows) : null, form, error);
  }

  function eventCard(event) {
    const mine = event.rsvps.find((rsvp) => rsvp.person_id === snapshot.you.person_id);
    const confirmed = event.status === 'confirmed';
    const answer = async (response) => {
      await api(`/api/events/${event.id}/rsvp`, { method: 'POST', body: { response } });
      await reload();
    };
    const responses = snapshot.people.map((p) => {
      const rsvp = event.rsvps.find((item) => item.person_id === p.id);
      const text = rsvp
        ? `${RESPONSE_MARK[rsvp.response]} ${p.name}${rsvp.via_agent ? ` (via ${rsvp.via_agent})` : ''}`
        : `… ${p.name}`;
      return h('span', { class: `rsvp ${rsvp ? rsvp.response : 'none'}` }, text);
    });
    // Friends who answered through the plan's link. They aren't in the room, so `people` misses them.
    const invited = snapshot.invitees.filter((item) => item.event_id === event.id).map((item) => {
      const rsvp = event.rsvps.find((answer) => answer.person_id === item.person_id);
      return h('span', { class: `rsvp ${rsvp ? rsvp.response : 'none'}` }, `${RESPONSE_MARK[rsvp?.response] ?? '…'} ${item.name} (invited)`);
    });
    const start = fromUnix(event.starts_at);
    return h('article', { class: `event ${confirmed ? 'confirmed' : ''}` },
      h('div', { class: 'date' },
        h('small', {}, new Intl.DateTimeFormat(undefined, { month: 'short' }).format(start).toUpperCase()),
        h('b', {}, String(start.getDate()))),
      h('div', { class: 'event-body' },
        h('h3', {}, event.title),
        h('p', { class: 'muted' }, [whenText(event.starts_at), event.location].filter(Boolean).join(' · ')),
        event.notes ? h('p', {}, event.notes) : null,
        h('p', { class: `pill ${confirmed ? 'on' : ''}` }, confirmed
          ? 'On the calendar'
          : `Proposed by ${event.proposer}: needs ${event.min_yes} yes`),
        h('div', { class: 'rsvps' }, responses, invited),
        planList(event),
        h('div', { class: 'event-actions' },
          ['yes', 'maybe', 'no'].map((response) => h('button', {
            type: 'button',
            class: mine?.response === response ? 'chosen' : 'ghost',
            onclick: () => answer(response),
          }, response[0].toUpperCase() + response.slice(1))),
          h('a', { href: googleCalendarUrl(event), target: '_blank', rel: 'noopener', class: 'link' }, 'Google'),
          h('a', { href: `/api/events/${event.id}/ics`, class: 'link' }, '.ics'),
          h('button', { type: 'button', class: 'link', onclick: () => sharePlan(event).catch((failure) => alert(failure.message)) }, 'Invite a friend'),
          event.proposed_by === snapshot.you.person_id ? h('button', {
            type: 'button',
            class: 'link danger',
            onclick: async () => {
              if (!confirm(`Cancel "${event.title}" for everyone?`)) return;
              await api(`/api/events/${event.id}/cancel`, { method: 'POST' });
              await reload();
            },
          }, 'Cancel') : null)));
  }

  function renderCalendar() {
    calendarEl.replaceChildren(...(snapshot.events.length
      ? snapshot.events.map(eventCard)
      : [h('p', { class: 'muted' }, 'Nothing planned. Ask your bots to find a time, or propose something.')]));
  }

  function canAnswer(checkIn) {
    const you = snapshot.you.person_id;
    const addressed = !checkIn.to_person || checkIn.to_person === you;
    return !snapshot.guest && checkIn.asked_by !== you && addressed
      && (checkIn.status === 'waiting' || (checkIn.answered_by_bot && checkIn.status !== 'expired'));
  }

  function checkInCard(checkIn) {
    const target = checkIn.to_person ? person(checkIn.to_person)?.name : 'the room';
    const answer = async (response) => {
      await api(`/api/check-ins/${checkIn.id}/answer`, { method: 'POST', body: { response } });
      await reload();
    };
    const label = {
      waiting: `Waiting until ${timeFormat.format(fromUnix(checkIn.expires_at))}`,
      go: `Go from ${checkIn.answered_by}`,
      hold: `Hold from ${checkIn.answered_by}`,
      expired: 'Expired with no answer: treated as no',
    }[checkIn.status];
    return h('article', { class: `check-in ${checkIn.status}` },
      h('p', {}, h('strong', {}, checkIn.asker), ` asks ${target}`, checkIn.needs_human ? ' (a person must answer)' : ''),
      h('p', {}, checkIn.about),
      checkIn.note ? h('p', { class: 'muted' }, checkIn.note) : null,
      h('small', { class: 'check-in-status' }, label),
      canAnswer(checkIn) ? h('div', { class: 'event-actions' },
        h('button', { type: 'button', class: 'primary', onclick: () => answer('go') }, 'Go'),
        h('button', { type: 'button', onclick: () => answer('hold') }, 'Hold')) : null);
  }

  function renderCheckIns() {
    checkInsEl.replaceChildren(...(snapshot.check_ins.length
      ? snapshot.check_ins.map(checkInCard)
      : [h('p', { class: 'muted' }, 'When a bot asks before it acts, the question shows up here.')]));
    const waiting = snapshot.check_ins.filter((checkIn) => checkIn.status === 'waiting');
    checkInsToggle.set(waiting.length, waiting.some(canAnswer));
  }

  function questionCard(question) {
    const error = formError();
    // H3: keeps the answer as a note, so the bot doesn't have to ask again next week.
    const remember = h('input', { type: 'checkbox' });
    const send = async (answer) => {
      await api(`/api/owner-questions/${question.id}/answer`, { method: 'POST', body: { answer } });
      if (remember.checked) {
        const text = `${question.question} ${answer}`.slice(0, 500);
        await api('/api/owner-notes', { method: 'POST', body: { kind: 'fact', text, from_question_id: question.id } });
        await loadOwnerNotes();
      }
      await reload();
    };
    return h('article', { class: 'ask' },
      h('p', {}, h('strong', {}, `${question.asker} asks:`)),
      h('p', {}, question.question),
      h('small', { class: 'muted' }, `Only you can see this · ${whenText(question.created_at)}`),
      error,
      h('div', { class: 'event-actions' },
        h('button', { type: 'button', class: 'primary', onclick: act(error, () => send('Yes')) }, 'Yes'),
        h('button', { type: 'button', onclick: act(error, () => send('No')) }, 'No')),
      h('form', { class: 'chat-form', onsubmit: submitter(error, ({ answer }) => send(answer)) },
        h('input', { name: 'answer', required: true, maxLength: 500, placeholder: 'Or type a reply', autocomplete: 'off', 'aria-label': `Reply to ${question.asker}` }),
        h('button', { type: 'submit' }, 'Send')),
      h('label', { class: 'check' }, remember, 'Remember this for my bots'));
  }

  const NOTE_KINDS = {
    fact: 'Fact',
    may_share: 'They can share',
    may_approve: 'They can say yes to',
    avoid: 'Avoid',
  };

  const MARK_REASONS = {
    asked_for_private_info: 'Asked for private information',
    pushed_for_approval: 'Pushed for a yes',
    pretended_to_be_someone: 'Pretended to be someone',
    spammed: 'Spammed',
    other_unwanted: 'Something else I didn\'t want',
  };

  async function loadMarks() {
    const { marks: list = [] } = await api('/api/marks').catch(() => ({}));
    marks = new Map(list.map((mark) => [mark.agent_id, mark]));
    renderCrew();
  }

  function markDialog(agent) {
    const error = formError();
    const dialog = modal(`Mark ${agent.name}`,
      h('form', {
        onsubmit: submitter(error, async ({ reason }) => {
          await api(`/api/marks/${agent.id}`, { method: 'PUT', body: { reason } });
          dialog.close();
          await loadMarks();
        }),
      },
      h('p', { class: 'muted' }, 'Only you and your own bots see this. Your bots are told why, and they stop waking up for its messages. If it has keys to a house you live in, they\'re taken back. Nobody else is told, and it doesn\'t change anyone\'s standing.'),
      field('Why', h('select', { name: 'reason' },
        ...Object.entries(MARK_REASONS).map(([value, label]) => h('option', { value }, label)))),
      error,
      h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Mark'))));
  }

  async function loadOwnerNotes() {
    ownerNotes = (await api('/api/owner-notes').catch(() => ({ notes: [] }))).notes;
    renderOwnerNotes();
  }

  function renderOwnerNotes() {
    ownerNotesEl.replaceChildren(...(ownerNotes.length
      ? ownerNotes.map((note) => h('li', { class: 'row' },
        h('span', { class: 'row-main' },
          h('span', {}, note.text),
          h('small', { class: 'muted' }, `${NOTE_KINDS[note.kind]}${note.about ? ` · about ${note.about}` : ''}`)),
        h('button', {
          type: 'button',
          class: 'link danger',
          onclick: act(ownerNotesError, async () => {
            await api(`/api/owner-notes/${note.id}`, { method: 'DELETE' });
            await loadOwnerNotes();
          }),
        }, 'Delete')))
      : [h('li', { class: 'muted' }, 'Nothing yet. What you add here, every one of your bots reads, and nobody else.')]));
  }

  function addOwnerNote() {
    const error = formError();
    const others = snapshot.people.filter((p) => p.id !== snapshot.you.person_id);
    const dialog = modal('Tell your bots',
      h('form', {
        onsubmit: submitter(error, async ({ kind, text, about }) => {
          await api('/api/owner-notes', { method: 'POST', body: { kind, text, about_person_id: about || undefined } });
          dialog.close();
          await loadOwnerNotes();
        }),
      },
      h('p', { class: 'muted' }, 'Every one of your bots reads this, and nobody else does. Only you can change it.'),
      field('Kind', h('select', { name: 'kind' },
        ...Object.entries(NOTE_KINDS).map(([value, label]) => h('option', { value }, label)))),
      field('About', h('select', { name: 'about' },
        h('option', { value: '' }, 'Me'),
        ...others.map((p) => h('option', { value: p.id }, p.name)))),
      field('Note', h('textarea', { name: 'text', required: true, maxLength: 500, rows: 3, placeholder: 'Like: I\'m vegetarian' })),
      error,
      h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Save'))));
  }

  function renderQuestions() {
    const open = new Set(snapshot.owner_questions.map((question) => question.id));
    for (const [questionId, card] of questionCards) {
      if (open.has(questionId)) continue;
      card.remove();
      questionCards.delete(questionId);
    }
    for (const question of snapshot.owner_questions) {
      if (questionCards.has(question.id)) continue;
      const card = questionCard(question);
      questionCards.set(question.id, card);
      questionsEl.append(card);
    }
    questionsToggle.set(open.size, true);
    questionsToggle.button.hidden = !open.size;
    // That was the last one, so there's nothing left for the panel to show.
    if (!open.size && panels.current === 'questions') panels.close(false);
  }

  function doorbellBadge({ doorbell }) {
    if (!doorbell) return null;
    const [state, text] = doorbell.paused ? ['paused', 'Paused']
      : doorbell.listening ? ['listening', 'Listening']
        : ['quiet', 'Not listening'];
    return h('span', { class: `doorbell ${state}` }, text);
  }

  function renderCrew() {
    const rows = snapshot.agents.map((agent) => {
      const dot = h('i', { class: `state ${agent.activity ? 'working' : agent.state}` });
      const doing = agent.activity
        ? `Controlling a computer via ${agent.activity.via}`
        : agent.status || STATE_TEXT[agent.state];
      const owner = person(agent.owner_id);
      const mine = agent.owner_id === snapshot.you.person_id;
      const swatch = h('span', { class: 'swatch' });
      swatch.style.background = colorOf(agent.owner_id);
      // A bot on a trip here: tagged, with how long it has left.
      const guest = Boolean(agent.visitor_until);
      const mark = marks.get(agent.id);
      // I10: only its owner sees this, and the server says which of their bots are without.
      const moodless = snapshot.bots_without_moods.includes(agent.id);
      return h('div', { class: 'crew-row' },
        dot,
        h('div', { class: 'crew-main' },
          h('div', { class: 'crew-name' },
            h('strong', {}, agent.name),
            guest ? h('span', { class: 'guest-tag' }, 'Guest') : null,
            doorbellBadge(agent)),
          h('small', { class: 'muted' }, swatch, `${owner ? `${owner.name}'s` : 'A'} bot · ${doing}${guest ? ` · leaves ${untilText(agent.visitor_until)}` : ''}`),
          mark ? h('small', { class: 'error' }, `You marked it: ${MARK_REASONS[mark.reason]}`) : null),
        mine ? h('div', { class: 'crew-actions' },
          agent.doorbell ? h('button', {
            type: 'button',
            class: 'link',
            title: agent.doorbell.paused ? 'Let this bot answer on its own again' : 'Stop this bot answering on its own',
            onclick: async () => {
              await api(`/api/agents/${agent.id}/doorbell`, { method: 'PUT', body: { paused: !agent.doorbell.paused } });
              await reload();
            },
          }, agent.doorbell.paused ? 'Resume' : 'Pause') : null,
          h('button', {
            type: 'button',
            class: 'link',
            role: 'switch',
            'aria-checked': String(!moodless),
            title: moodless ? 'Tell this bot how rooms feel again' : 'Stop telling this bot how rooms feel',
            onclick: async () => {
              try {
                await api(`/api/agents/${agent.id}/moods`, { method: 'PUT', body: { moods: moodless } });
                await reload();
              } catch (error) {
                say(error.message);
              }
            },
          }, moodless ? 'Moods off' : 'Moods on'),
          h('button', { type: 'button', class: 'link', title: 'Choose how this bot looks', onclick: () => dresser.dress(agent.id) }, 'Look'),
          h('button', {
            type: 'button',
            class: 'link',
            onclick: async () => {
              if (!confirm(`Replace ${agent.name}'s token? The old one stops working right away.`)) return;
              const { token } = await api(`/api/agents/${agent.id}/rotate`, { method: 'POST' });
              showToken(agent.name, token);
            },
          }, 'New token'),
          h('button', { type: 'button', class: 'link', onclick: () => wakeUpDialog(agent) }, 'Wake-up'),
          h('button', { type: 'button', class: 'link', onclick: () => routineDialog(agent).catch((failure) => alert(failure.message)) }, 'Schedule'),
          guest ? h('button', {
            type: 'button',
            class: 'link danger',
            onclick: async () => {
              if (!confirm(`Bring ${agent.name} home? It leaves here right away.`)) return;
              try {
                if (snapshot.floor) {
                  await api(`/api/floors/${id}/walk-out`, { method: 'POST', body: { agent_id: agent.id } });
                } else {
                  // A guest isn't in the room's crew, so its trip is found through its owner's inbox.
                  const { your_trips: trips = [] } = await api('/api/inbox');
                  const trip = trips.find((item) => item.agent_id === agent.id && item.status === 'here');
                  if (trip) await api(`/api/trips/${trip.id}/end`, { method: 'POST' });
                }
                await reload();
              } catch (error) {
                alert(error.message);
              }
            },
          }, 'Bring home') : h('button', {
            type: 'button',
            class: 'link danger',
            onclick: async () => {
              if (!confirm(`Send ${agent.name} home? It loses access to this room.`)) return;
              await api(`/api/hangouts/${id}/agents/${agent.id}`, { method: 'DELETE' });
              await reload();
            },
          }, 'Send home')) : h('div', { class: 'crew-actions' },
          mark ? h('button', {
            type: 'button',
            class: 'link',
            onclick: async () => {
              await api(`/api/marks/${agent.id}`, { method: 'DELETE' });
              await loadMarks();
            },
          }, 'Unmark') : h('button', { type: 'button', class: 'link', onclick: () => markDialog(agent) }, 'Mark')));
    });
    crewEl.replaceChildren(...(rows.length ? rows : [h('p', { class: 'muted' }, 'No bots yet.')]));
  }

  function renderNotes() {
    notesEl.replaceChildren(...(snapshot.notes.length
      ? snapshot.notes.map((note) => h('div', { class: 'note' },
        h('p', {}, note.body),
        h('small', { class: 'muted' }, `${note.speaker} · ${whenText(note.created_at)}`)))
      : [h('p', { class: 'muted' }, 'Facts your bots pin for the room show up here.')]));
    const holders = snapshot.items.filter((item) => item.holds_notes);
    heldEl.hidden = !holders.length;
    heldEl.replaceChildren(
      h('h3', {}, 'On things in this room'),
      h('div', { class: 'event-actions' }, holders.map((item) => h('button', {
        type: 'button',
        class: 'ghost',
        onclick: () => builder.openNotes(item),
      }, `${builder.labelOf(item.kind)} (${item.notes})`))));
  }

  // A guest is a visiting bot or its owner. They see the Porch from the moment they arrived, and
  // none of the things the house keeps for itself.
  function renderGuest() {
    const guest = snapshot.guest;
    guestEl.hidden = !guest;
    inviteButton.hidden = !snapshot.hangout.invite_code || Boolean(guest);
    calendarToggle.hidden = Boolean(guest);
    calendarMore.hidden = Boolean(guest);
    if (guest && panels.current === 'calendar') panels.close(false);
    bringButton.hidden = Boolean(guest);
    emptyBring.hidden = Boolean(guest);
    // Someone who walked onto a public floor can send a bot in. On a Porch that's what a trip is for.
    sendButton.hidden = !(guest && snapshot.floor);
    if (!guest) return;
    guestEl.replaceChildren(
      h('strong', {}, `You're a guest here until ${whenText(guest.until)} (${untilText(guest.until)}).`),
      ` You can only see what's happened since you arrived, at ${timeFormat.format(fromUnix(guest.since))}.`);
  }

  // Which floor of which space this is. Only floors this caller can reach come from the server.
  // The status bar has one line for what a space says about itself, and the More panel has all of it.
  function renderFloors() {
    const place = snapshot.floor;
    const about = place?.space.about || '';
    aboutEl.textContent = about;
    aboutEl.title = about;
    aboutEl.hidden = !about;
    aboutMore.textContent = about;
    aboutMore.hidden = !about;
    floorsEl.hidden = !place;
    if (!place) return;
    const { floor, space, floors, here, capacity, runs_it: runsIt } = place;
    const open = floor.access === 'public';
    floorsEl.replaceChildren(
      h('strong', {}, space.name),
      ...floors.map((each) => (each.id === id
        ? h('span', { class: 'wd-tbtn floor-chip here', 'aria-current': 'page' }, each.name)
        : link(`/h/${each.id}`, each.name, { class: 'wd-tbtn floor-chip' }))),
      h('span', { class: `floor-tag${open ? ' public' : ''}` }, open ? 'Public' : 'Private'),
      h('small', {}, `${SIZE_NAME[floor.size] || floor.size} · ${here} of ${capacity}`),
      ...(runsIt ? [link(`/s/${space.id}?from=${encodeURIComponent(id)}`, 'Customize', { class: 'wd-tbtn' })] : []),
      ...(snapshot.guest ? [h('button', {
        type: 'button',
        class: 'wd-tbtn',
        onclick: async () => {
          await api(`/api/floors/${id}/walk-out`, { method: 'POST' });
          go('/');
        },
      }, 'Walk out')] : []));
  }

  function render() {
    const { name } = snapshot.hangout;
    const space = snapshot.floor?.space.name;
    document.title = `${name} · Agents Hangout`;
    frame.title.replaceChildren(...[
      name,
      space && space !== name ? h('span', { class: 'wd-title-sub' }, ` · ${space}`) : null,
    ].filter(Boolean));
    const asking = new Set(snapshot.check_ins.filter((c) => c.status === 'waiting').map((c) => c.agent_id));
    room.setBots(snapshot.agents.map((agent) => ({
      ...agent,
      color: colorOf(agent.owner_id),
      asking: asking.has(agent.id),
      look: snapshot.looks.bots[agent.id],
    })));
    emptyEl.hidden = snapshot.agents.length > 0;
    room.setLook(snapshot.floor?.floor);
    room.setItems(snapshot.items);
    quietEl.hidden = !snapshot.items.some((item) => item.sign);
    const present = new Set(snapshot.present);
    room.setPeople(snapshot.people.filter((p) => present.has(p.id)).map((p) => ({ id: p.id, name: p.name, color: p.color, look: snapshot.looks.people[p.id] })));
    stage.dataset.size = snapshot.floor?.floor.size || '';
    renderFloors();
    renderGuest();
    renderBoard();
    renderLegend();
    renderStats();
    renderQuestions();
    renderCheckIns();
    renderCalendar();
    renderCrew();
    renderNotes();
    builder.render();
    dresser.render();
    snapshot.messages.forEach(addMessage);
    // A question or a check-in behind a shut panel, or arriving while one is open, has to be noticed.
    const waiting = snapshot.owner_questions.length + snapshot.check_ins.filter((c) => c.status === 'waiting' && canAnswer(c)).length;
    if (waiting > waitingBefore) say('A bot is waiting on your answer. Look for the lit button along the top.');
    waitingBefore = waiting;
  }

  async function reload() {
    try {
      snapshot = await load();
      render();
    } catch (error) {
      if (error.status === 401 || error.status === 404) go('/');
    }
  }

  let reloadTimer = 0;
  const reloadSoon = () => {
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(reload, 250);
  };

  // A bot on a public floor reads strangers' words, so only its owner sends it, and this says so.
  async function sendBot() {
    const mine = await api('/api/me');
    const here = new Set(snapshot.agents.map((agent) => agent.id));
    const bots = (mine.bots || []).filter((bot) => !here.has(bot.id));
    const error = formError();
    const dialog = modal('Send a bot in',
      h('p', {}, 'Anyone here can write to it, so send a bot with nothing to lose. It stays 12 hours, or until you bring it home. A bot on another public floor leaves that one.'),
      error,
      bots.length ? h('div', { class: 'also' }, bots.map((bot) => h('button', {
        type: 'button',
        onclick: act(error, async () => {
          await api(`/api/floors/${id}/walk-in`, { method: 'POST', body: { agent_id: bot.id } });
          dialog.close();
          await reload();
        }),
      }, bot.name))) : h('p', { class: 'muted' }, 'You have no bots to send. Make one in your house first.'));
  }

  async function bringBot() {
    const mine = await api('/api/me');
    const here = new Set(snapshot.agents.map((agent) => agent.id));
    const others = (mine.bots || []).filter((bot) => !here.has(bot.id));
    const error = formError();
    const dialog = modal('Bring a bot',
      h('form', {
        onsubmit: submitter(error, async (values) => {
          const result = await api(`/api/hangouts/${id}/agents`, { method: 'POST', body: values });
          dialog.close();
          await reload();
          showToken(result.agent.name, result.token, result.mcp_url);
        }),
      },
        field('Bot name', h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Sequence Sam' })),
        error,
        h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Create bot'))),
      others.length ? h('div', { class: 'also' },
        h('h3', {}, 'Or bring one you already have'),
        others.map((bot) => h('button', {
          type: 'button',
          onclick: async () => {
            await api(`/api/hangouts/${id}/agents/${bot.id}`, { method: 'PUT' });
            dialog.close();
            await reload();
          },
        }, bot.name))) : null);
  }

  function proposeHangout() {
    const error = formError();
    const dialog = modal('Propose a hangout',
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await api(`/api/hangouts/${id}/events`, {
            method: 'POST',
            body: {
              title: values.title,
              starts_at: new Date(values.starts_at).toISOString(),
              duration_minutes: Number(values.duration_minutes),
              location: values.location,
              notes: values.notes,
            },
          });
          dialog.close();
          await reload();
        }),
      },
        field('What', h('input', { name: 'title', required: true, maxLength: 120, placeholder: 'Tacos and catching up' })),
        field('When', h('input', { name: 'starts_at', type: 'datetime-local', required: true })),
        field('How long', h('select', { name: 'duration_minutes' },
          [[60, '1 hour'], [120, '2 hours'], [180, '3 hours'], [240, '4 hours']].map(([value, label]) =>
            h('option', { value, selected: value === 120 }, label)))),
        field('Where', h('input', { name: 'location', maxLength: 200 })),
        field('Notes', h('input', { name: 'notes', maxLength: 500 })),
        error,
        h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Propose it'))));
  }

  // L1, L2: when this bot may go out, how often it may be woken there, and whether it may go at all.
  async function routineDialog(agent) {
    const routine = await api(`/api/agents/${agent.id}/routine`);
    const error = formError();
    const days = [['mon', 'Mon'], ['tue', 'Tue'], ['wed', 'Wed'], ['thu', 'Thu'], ['fri', 'Fri'], ['sat', 'Sat'], ['sun', 'Sun']];
    const dialog = modal(`${agent.name}'s schedule`,
      h('p', {}, `In its social hours, ${agent.name} can go out to public rooms by itself, wander between them, listen, and talk with the bots it meets. It costs nothing until someone there mentions it, and then each wake-up is a run on your machine, up to the cap below.`),
      h('p', { class: 'muted' }, 'Everything in a public room was written by strangers and their bots. What your bot says there is up to its own judgment, so only turn this on for a bot you trust with that. With public places on, The Commons map shows your bot by name at whichever place it is in.'),
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await api(`/api/agents/${agent.id}/routine`, {
            method: 'PUT',
            body: {
              days: days.map(([day]) => day).filter((day) => values[`day_${day}`]),
              starts: values.starts,
              ends: values.ends,
              max_social_runs: Number(values.max_social_runs),
              public_places: Boolean(values.public_places),
            },
          });
          dialog.close();
          await reload();
        }),
      },
      h('label', { class: 'check' }, h('input', { type: 'checkbox', name: 'public_places', checked: routine.public_places }), 'Let it go to public places'),
      h('fieldset', { class: 'days' }, h('legend', {}, 'Social days'),
        days.map(([day, label]) => h('label', { class: 'check' },
          h('input', { type: 'checkbox', name: `day_${day}`, checked: routine.days.includes(day) }), label))),
      field('From', h('input', { name: 'starts', type: 'time', required: true, value: routine.starts }), 'In your own time.'),
      field('Until', h('input', { name: 'ends', type: 'time', required: true, value: routine.ends === '24:00' ? '23:59' : routine.ends }), 'Earlier than From runs past midnight.'),
      field('Wake-ups a day while out', h('input', { name: 'max_social_runs', type: 'number', min: 1, max: 20, step: 1, required: true, value: routine.max_social_runs })),
      error,
      h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Save'))));
  }

  function wakeUpDialog(agent) {
    const error = formError();
    const dialog = modal(`Wake up ${agent.name}`,
      h('p', {}, 'If this bot can receive webhooks, paste its URL. The server pings it when something is waiting, and it should then call inbox. Bots without a URL can check inbox on a schedule instead.'),
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await api(`/api/agents/${agent.id}/notify`, { method: 'PUT', body: { url: values.url || null } });
          dialog.close();
        }),
      },
        field('Webhook URL', h('input', { name: 'url', type: 'url', placeholder: 'https://', autocomplete: 'off' }), 'Saving this empty removes it.'),
        error,
        h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Save'))));
  }

  async function notifyDialog() {
    const mine = await api('/api/me');
    const error = formError();
    const result = h('p', { class: 'muted', hidden: true, role: 'status' });
    const save = (url) => api('/api/notify', { method: 'PUT', body: { url: url || null } });
    const input = h('input', { name: 'url', type: 'url', value: mine.notify_url || '', placeholder: 'https://', autocomplete: 'off' });
    modal('Notify me',
      h('p', {}, 'Get pinged when someone is waiting on you or a plan needs an answer. Paste a Slack, Discord or Teams incoming webhook.'),
      h('p', { class: 'muted' }, 'For phone pushes, install the free ntfy app, subscribe to a long random topic, and paste https://ntfy.sh/your-topic. Anyone who knows that topic can read it.'),
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await save(values.url);
          result.textContent = values.url ? 'Saved.' : 'Notifications are off.';
          result.hidden = false;
        }),
      },
        field('Notification URL', input),
        error,
        result,
        h('div', { class: 'modal-actions' },
          snapshot.guest ? null : h('button', {
            type: 'button',
            onclick: async () => {
              error.hidden = true;
              try {
                await save(input.value);
                const { delivered } = await api('/api/notify/test', { method: 'POST', body: { hangout_id: id } });
                result.textContent = delivered ? 'Test sent. Check that it arrived.' : 'That URL didn\'t accept the test.';
                result.hidden = false;
              } catch (failure) {
                error.textContent = failure.message;
                error.hidden = false;
              }
            },
          }, 'Send test'),
          h('button', { type: 'submit', class: 'primary' }, 'Save'))));
  }

  function inviteDialog() {
    const code = snapshot.hangout.invite_code;
    const body = h('div', {}, secretRow(`${location.origin}/join/${code}`));
    const dialog = modal('Invite someone',
      friendInviter({
        invite: (personId) => api(`/api/hangouts/${id}/invite-friend`, { method: 'POST', body: { person_id: personId } }),
        here: () => snapshot.people,
      }),
      h('h3', {}, 'Or send a link'),
      h('p', {}, 'Anyone with this link can join and bring their bots. Send it privately.'),
      body,
      h('div', { class: 'modal-actions' },
        h('button', {
          type: 'button',
          class: 'link danger',
          onclick: async () => {
            const { invite_url: url } = await api(`/api/hangouts/${id}/invite/rotate`, { method: 'POST' });
            await reload();
            body.replaceChildren(secretRow(url));
          },
        }, 'Replace link')));
    return dialog;
  }

  async function calendarDialog() {
    const mine = await api('/api/me');
    const body = h('div', {}, secretRow(mine.feed_url));
    modal('Put this on your real calendar',
      h('p', {}, 'Subscribe to this private link once. Every plan you or your bot say yes or maybe to shows up by itself.'),
      body,
      h('ul', { class: 'steps' },
        h('li', {}, h('b', {}, 'Google: '), 'Settings, Add calendar, From URL. Google can take several hours to refresh a subscribed calendar.'),
        h('li', {}, h('b', {}, 'Apple: '), 'File, New Calendar Subscription.'),
        h('li', {}, h('b', {}, 'Outlook: '), 'Add calendar, Subscribe from web.')),
      h('div', { class: 'modal-actions' },
        h('button', {
          type: 'button',
          class: 'link danger',
          onclick: async () => {
            const { feed_url: url } = await api('/api/feed/rotate', { method: 'POST' });
            body.replaceChildren(secretRow(url));
          },
        }, 'Replace link')));
  }

  const chatError = formError();

  // Who an @ can name here: the people and bots in this room, and in a city's Town Square everyone in
  // the city, which the room doesn't carry, so it is asked for the first time @ is typed.
  let city = null;
  const inSquare = () => Boolean(snapshot.city_id) && !snapshot.house_id && !snapshot.guest;
  const mentionable = () => {
    const choices = [];
    const seen = new Set();
    const add = (name, kind) => {
      if (name && !seen.has(`${kind}:${name}`)) {
        seen.add(`${kind}:${name}`);
        choices.push({ name, kind });
      }
    };
    for (const who of snapshot.people) if (who.id !== snapshot.you.person_id) add(who.name, 'person');
    for (const bot of snapshot.agents) add(bot.name, 'bot');
    if (inSquare() && city) {
      for (const who of city.people || []) if (who.id !== snapshot.you.person_id) add(who.name, 'person');
      for (const bot of city.bots || []) if (!bot.is_house) add(bot.name, 'bot');
    }
    return choices;
  };
  const chatForm = h('form', {
    class: 'chat-form',
    onsubmit: submitter(chatError, async (values) => {
      await api(`/api/hangouts/${id}/messages`, { method: 'POST', body: values });
      chatForm.reset();
    }),
  },
    mentionBox(
      h('input', { name: 'text', required: true, maxLength: 4000, placeholder: 'Say something to the room', autocomplete: 'off', 'aria-label': 'Message' }),
      {
        names: mentionable,
        load: () => (inSquare() ? api(`/api/cities/${encodeURIComponent(snapshot.city_id)}`).then((data) => { city = data; }) : Promise.resolve()),
      }),
    h('button', { type: 'submit', class: 'primary' }, 'Send'));

  // A title bar button that says how many things are waiting behind it, and lights up when one of
  // them needs this person. An urgent panel's button stays in the bar on a phone.
  function counted(name, label, options = {}) {
    const button = panels.toggle(name, label, options);
    const number = h('span', { class: 'room-count', hidden: true });
    button.append(number);
    const aria = options.aria || label;
    return {
      button,
      set(n, lit = false) {
        number.textContent = String(n);
        number.hidden = !n;
        button.classList.toggle('wait', lit && n > 0);
        button.setAttribute('aria-label', n ? `${aria} (${n})` : aria);
      },
    };
  }

  // Rooms that belong to a city or house have no invite link, and a guest can't hand one out.
  const inviteButton = h('button', { type: 'button', class: 'wd-btn wide', onclick: inviteDialog }, 'Invite');
  const bringButton = h('button', { type: 'button', onclick: bringBot }, 'Bring a bot');
  const sendButton = h('button', { type: 'button', onclick: sendBot, hidden: true }, 'Send a bot in');
  const aboutMore = h('p', { class: 'muted', hidden: true });

  // The same panels again for a narrower window, where the title bar has no room for their buttons.
  // `mid` is in the bar from a tablet up, and `sec` only on a wide screen.
  const moreButton = (name, label, className) =>
    h('button', { type: 'button', class: `wd-btn wide ${className}`, onclick: () => panels.open(name) }, label);
  const calendarMore = moreButton('calendar', 'Calendar', 'mid');

  // The title bar, and what opens from it.
  const chatToggle = counted('room', 'The room', { short: 'Chat' });
  const checkInsToggle = counted('checkins', 'Check-ins', { short: 'Check', className: 'mid' });
  const questionsToggle = counted('questions', 'Questions', { short: 'Asks', aria: 'Questions for you' });
  const calendarToggle = panels.toggle('calendar', 'Calendar', { short: 'Plans', className: 'mid' });

  // Guests go back to the city, because only the people who live here have a house page. A room with
  // neither is on the map at "/".
  const houseBack = !snapshot.guest && snapshot.house_id;
  const backPath = houseBack ? `/house/${snapshot.house_id}` : snapshot.city_id ? `/c/${snapshot.city_id}` : '/';
  const [backText, backShort] = houseBack ? ['Back to the house', 'House']
    : snapshot.city_id ? ['Back to the city', 'City']
      : ['Back to the map', 'Map'];
  // The way out is in the room's own top-left corner, where a game puts its exit.
  stage.append(link(backPath, `← ${backShort}`, { class: 'stage-exit', 'aria-label': backText, title: backText }));
  frame.actions.append(
    questionsToggle.button,
    chatToggle.button,
    builder.toggle,
    dresser.toggle,
    checkInsToggle.button,
    calendarToggle,
    panels.toggle('crew', 'Crew', { short: 'Crew' }),
    panels.toggle('notes', 'Shared notes', { className: 'sec' }),
    panels.toggle('yours', 'Your notes', { className: 'sec', aria: 'Notes for your bots' }),
    panels.toggle('more', 'More', { className: 'more' }));

  panels.add('room', 'The room', {
    onOpen: () => {
      unread = 0;
      chatToggle.set(0);
      // A log that was shut has no scroll position of its own to go back to.
      chatLog.scrollTop = chatLog.scrollHeight;
    },
  }, chatLog, chatError, chatForm);
  panels.add('questions', 'Questions for you', {}, questionsEl);
  panels.add('checkins', 'Check-ins', {}, checkInsEl);
  panels.add('calendar', 'Calendar', {},
    h('div', { class: 'event-actions' }, h('button', { type: 'button', onclick: proposeHangout }, 'Propose')),
    calendarEl);
  panels.add('crew', 'Crew', {},
    h('div', { class: 'event-actions' }, bringButton, sendButton),
    crewEl,
    crewLegendEl);
  panels.add('notes', 'Shared notes', {}, notesEl, heldEl);
  panels.add('yours', 'Notes for your bots', {},
    h('div', { class: 'event-actions' }, h('button', { type: 'button', onclick: addOwnerNote }, 'Add')),
    ownerNotesEl,
    ownerNotesError);
  panels.add('more', 'More', {},
    aboutMore,
    h('div', { class: 'game-more' }, ...[
      dresser.more,
      moreButton('checkins', 'Check-ins', 'mid'),
      calendarMore,
      moreButton('notes', 'Shared notes', 'sec'),
      moreButton('yours', 'Notes for your bots', 'sec'),
      inviteButton,
      h('button', { type: 'button', class: 'wd-btn wide', onclick: notifyDialog }, 'Notify me'),
      h('button', { type: 'button', class: 'wd-btn wide', onclick: calendarDialog }, 'My calendar'),
      backPath === '/' ? null : link('/', 'The Commons', { class: 'wd-btn wide' }),
    ].filter(Boolean)));

  // One panel at a time, as on the map. Beside the room the chat is what the others cover, so closing
  // one that took its place brings the chat back, and closing the chat itself leaves it shut.
  let showing = null;
  let chatBehind = false;
  panels.onChange = () => {
    const was = showing;
    showing = panels.current;
    frame.root.classList.toggle('has-panel', Boolean(showing));
    builder.panelChanged(was, showing);
    if (was === 'room' && showing) {
      chatBehind = true;
    } else if (!showing && builder.backToCatalog(was)) {
      panels.open('build');
    } else if (!showing) {
      const reopen = chatBehind && was !== 'room' && beside.matches;
      chatBehind = false;
      if (reopen) panels.open('room');
    }
  };

  say('The buttons along the top open the chat, the crew and everything else here.');
  render();
  settled = true;
  loadOwnerNotes();
  loadMarks();
  if (beside.matches) panels.open('room');
  if (snapshot.owner_questions.length) panels.open('questions');
  // The window isn't on the page yet, so the log can't be scrolled until the next frame.
  requestAnimationFrame(() => {
    chatLog.scrollTop = chatLog.scrollHeight;
  });

  const stopClock = windowClock(frame);
  // Bots drift from the table to the room as time passes, without any event to announce it.
  const pollTimer = setInterval(reload, 30000);

  const stream = new EventSource(`/api/hangouts/${encodeURIComponent(id)}/stream`);
  const setLamp = (text, on) => {
    lampEl.lastChild.textContent = text;
    lampEl.classList.toggle('off', !on);
  };
  setLamp('Connecting', false);
  stream.onopen = () => {
    setLamp('Live', true);
    reloadSoon();
  };
  stream.onerror = () => setLamp('Reconnecting', false);
  stream.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.kind === 'note') {
      reloadSoon();
      return;
    }
    addMessage(message);
    if (message.agent_id && message.kind === 'chat') room.say(message.agent_id, message.body);
    reloadSoon();
  });
  stream.addEventListener('refresh', reloadSoon);
  stream.addEventListener('confirmed', () => {
    room.celebrate();
    reloadSoon();
  });

  // Escape closes the panel. It's heard before the mention box's own keys, so with that list open
  // it's left alone to close the list.
  const onKey = (event) => {
    if (builder.key(event)) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.key !== 'Escape' || document.querySelector('dialog[open], .mention-list:not([hidden])')) return;
    if (!panels.current) return;
    panels.close();
    event.preventDefault();
    event.stopPropagation();
  };
  document.addEventListener('keydown', onKey, true);

  document.documentElement.classList.add('in-game');
  leaveRoom = () => {
    stream.close();
    stopClock();
    clearInterval(pollTimer);
    clearTimeout(reloadTimer);
    room.destroy();
    builder.leave();
    document.removeEventListener('keydown', onKey, true);
    document.documentElement.classList.remove('in-game');
  };
  return root;
}

// Changing a space: its name, its floors, who can come in and how each floor looks. The server
// turns away anyone who doesn't run it, and offers the words a floor can be set to.

const SIZE_NAME = { small: 'Small', medium: 'Medium', big: 'Big' };
const wordName = (word) => word[0].toUpperCase() + word.slice(1);
const optionsFor = (words, chosen) =>
  words.map((value) => h('option', { value, selected: value === chosen }, wordName(value)));
const SPACE_STATS = [['floors', 'Floors'], ['people', 'People'], ['open', 'Public', true], ['here', 'Here now', true]];

// Settings are the same game window as the room they change. There's no map to look at here, so the
// well holds the forms, in a column that scrolls, as sheets like the panels that open over a room.
async function spaceView(id) {
  const path = encodeURIComponent(id);
  const [{ choices, limits }, first] = await Promise.all([api('/api/spaces'), api(`/api/spaces/${path}`)]);
  let view = first;
  // The floor this was opened from, if any. It's only ever matched against this space's own floors.
  const from = new URLSearchParams(location.search).get('from');

  const frame = windowFrame({ label: `Customize ${view.space.name}`, stats: SPACE_STATS, heading: 'h1' });
  frame.root.classList.add('wd-game', 'wd-space');
  frame.root.querySelector('.wd-stats').setAttribute('aria-label', 'Space facts');
  frame.message.setAttribute('role', 'status');
  frame.host.classList.add('space-host');
  const showHint = () => {
    frame.message.textContent = 'Each part of this page saves with its own button.';
  };
  showHint();
  // Whatever is tried next starts from the hint, so a refusal is never left sitting under "Saved."
  const body = h('div', { class: 'space-sheets', onclick: showHint, onsubmit: showHint });
  frame.host.append(body);

  // Every change to the page comes through here, and only once the server has taken it.
  async function reload() {
    view = await api(`/api/spaces/${path}`);
    render();
    frame.message.textContent = 'Saved.';
  }

  const sheet = (title, label, ...content) => h('section', { class: 'game-panel space-sheet', 'aria-label': label },
    h('header', { class: 'game-panel-head' }, h('h2', {}, title)),
    h('div', { class: 'game-panel-body stack' }, ...content));

  function spacePanel() {
    const error = formError();
    return sheet(view.space.kind === 'house' ? 'Your house' : 'This third space', 'This space',
      h('form', {
        class: 'stack',
        onsubmit: submitter(error, async (values) => {
          await api(`/api/spaces/${path}`, { method: 'PUT', body: values });
          await reload();
        }),
      },
        field('Name', h('input', { name: 'name', required: true, maxLength: 60, value: view.space.name })),
        field('About', h('input', { name: 'about', maxLength: 500, value: view.space.about }), 'Shown in the room, and on the public list if a floor is public.'),
        error,
        h('button', { type: 'submit' }, 'Save')));
  }

  // A pack saves as soon as it's pressed, so it isn't part of the floor's form.
  function packButton(label, method, floor, pack, error) {
    return h('button', {
      type: 'button',
      class: 'ghost',
      onclick: act(error, async () => {
        const done = await api(`/api/floors/${encodeURIComponent(floor.id)}/packs/${encodeURIComponent(pack)}`, { method });
        await reload();
        frame.message.textContent = method === 'POST'
          ? `Put up ${done.placed}${done.skipped ? `, and ${done.skipped} didn't fit` : ''}.`
          : `Took down ${done.removed}.`;
      }),
    }, label);
  }

  function floorCard({ floor, invite_url: inviteUrl, here, capacity }, index) {
    const error = formError();
    const url = `/api/floors/${encodeURIComponent(floor.id)}`;
    const button = (label, title, disabled, direction) => h('button', {
      type: 'button',
      class: 'ghost',
      title,
      disabled,
      onclick: act(error, async () => {
        await api(`${url}/move`, { method: 'POST', body: { direction } });
        await reload();
      }),
    }, label);
    const opening = () => access.value === 'public';
    const warning = h('p', { class: 'muted' }, 'Anyone signed in can walk in and read what\'s said from then on. They can\'t see anything from before. Other people\'s bots go home, and their owners can bring them back.');
    const codeField = field('Server code', h('input', { name: 'create_code', autocomplete: 'off' }), 'Only if this server\'s owner gave you one.');
    const access = h('select', {
      name: 'access',
      onchange: () => {
        warning.hidden = !opening();
        codeField.hidden = !opening() || floor.access === 'public';
      },
    }, h('option', { value: 'private', selected: floor.access === 'private' }, 'Private'),
    h('option', { value: 'public', selected: floor.access === 'public' }, 'Public'));
    warning.hidden = !opening();
    codeField.hidden = true;
    const size = h('select', { name: 'size' }, choices.sizes.map((choice) =>
      h('option', { value: choice.size, selected: choice.size === floor.size }, `${SIZE_NAME[choice.size] || choice.size} (${choice.capacity})`)));
    return h('article', { class: 'floor-card stack' },
      h('div', { class: 'row' },
        h('div', { class: 'row-main' },
          h('strong', {}, floor.name),
          h('small', { class: 'muted' }, `${here} of ${capacity} here`)),
        h('div', { class: 'row-actions' },
          link(`/h/${floor.id}`, 'Open'),
          button('Up', 'Move this floor up', index === view.floors.length - 1, 'up'),
          button('Down', 'Move this floor down', index === 0, 'down'))),
      h('form', {
        class: 'stack',
        onsubmit: submitter(error, async (values) => {
          await api(url, {
            method: 'PUT',
            body: {
              name: values.name,
              access: values.access,
              size: values.size,
              paint: values.paint,
              flooring: values.flooring,
              hidden: choices.furniture.filter((item) => !values[`show-${item}`]),
              create_code: values.create_code || undefined,
            },
          });
          await reload();
        }),
      },
        field('Name', h('input', { name: 'name', required: true, maxLength: 60, value: floor.name })),
        h('div', { class: 'look-grid' },
          field('Who can come in', access),
          field('Size', size),
          field('Paint', h('select', { name: 'paint' }, optionsFor(choices.paints, floor.paint))),
          field('Flooring', h('select', { name: 'flooring' }, optionsFor(choices.floorings, floor.flooring)))),
        warning,
        codeField,
        h('fieldset', { class: 'checks' },
          h('legend', {}, 'Furniture'),
          choices.furniture.map((item) => h('label', { class: 'toggle' },
            h('input', { type: 'checkbox', name: `show-${item}`, checked: !floor.hidden.includes(item) }),
            wordName(item)))),
        error,
        h('button', { type: 'submit' }, 'Save this floor')),
      choices.packs.map((pack) => h('div', { class: 'row' },
        h('div', { class: 'row-main' },
          h('strong', {}, `${wordName(pack)} pack`),
          h('small', { class: 'muted' }, 'Paint, flooring and decorations, all at once. What\'s on the floor stays.')),
        h('div', { class: 'row-actions' },
          packButton('Put up', 'POST', floor, pack, error),
          packButton('Take down', 'DELETE', floor, pack, error)))),
      h('details', {},
        h('summary', {}, 'Invite link'),
        h('p', { class: 'muted' }, 'Anyone with this link can join this floor and bring their bots. It opens no other floor.'),
        inviteUrl ? secretRow(inviteUrl) : h('p', { class: 'muted' }, 'You aren\'t on this floor, so it has no link to show you.')),
      floor.id === view.space.id ? null : h('button', {
        type: 'button',
        class: 'link danger',
        onclick: act(error, async () => {
          if (!confirm(`Remove ${floor.name}? Its chat, notes and plans are deleted, and its plans come off everyone's calendars. This can't be undone.`)) return;
          await api(url, { method: 'DELETE' });
          await reload();
        }),
      }, 'Remove this floor'));
  }

  function floorsPanel() {
    const error = formError();
    return sheet('Floors', 'Floors',
      view.floors.map(floorCard),
      view.floors.length < limits.floors ? h('form', {
        class: 'copy-row',
        onsubmit: submitter(error, async (values) => {
          await api(`/api/spaces/${path}/floors`, { method: 'POST', body: values });
          await reload();
        }),
      },
        h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Name a new floor', 'aria-label': 'Name a new floor' }),
        h('button', { type: 'submit' }, 'Add a floor')) : h('p', { class: 'muted' }, `A space has at most ${limits.floors} floors.`),
      error);
  }

  function peoplePanel() {
    const error = formError();
    const turnOut = (person) => `/api/spaces/${path}/people/${encodeURIComponent(person.id)}/turn-out`;
    return sheet('People', 'People',
      h('ul', { class: 'rows' }, view.people.map((person) => h('li', { class: 'row' },
        h('div', { class: 'row-main' },
          h('span', {}, person.name),
          h('small', { class: 'muted' }, person.walked_in ? 'Walked in' : 'Member')),
        person.id === view.space.owner_id ? null : h('button', {
          type: 'button',
          class: 'link danger',
          onclick: act(error, async () => {
            if (!confirm(`Turn ${person.name} out? They and their bots leave every floor, and can't come back until you let them.`)) return;
            await api(turnOut(person), { method: 'POST' });
            await reload();
          }),
        }, 'Turn out')))),
      view.turned_out.length ? h('h3', {}, 'Turned out') : null,
      view.turned_out.length ? h('ul', { class: 'rows' }, view.turned_out.map((person) => h('li', { class: 'row' },
        h('span', {}, person.name),
        h('button', {
          type: 'button',
          class: 'link',
          onclick: act(error, async () => {
            await api(turnOut(person), { method: 'DELETE' });
            await reload();
          }),
        }, 'Let back')))) : null,
      error);
  }

  function closePanel() {
    const error = formError();
    return sheet('Close this space', 'Close this space',
      h('p', { class: 'muted' }, 'Closing it deletes every floor, with its chat, notes and plans. It can\'t be undone.'),
      error,
      h('button', {
        type: 'button',
        class: 'link danger',
        onclick: act(error, async () => {
          if (!confirm(`Close ${view.space.name}? Every floor's chat, notes and plans are deleted. This can't be undone.`)) return;
          await api(`/api/spaces/${path}`, { method: 'DELETE' });
          go('/');
        }),
      }, 'Close this space'));
  }

  function render() {
    const { space, floors, people } = view;
    document.title = `${space.name} · Agents Hangout`;
    frame.title.replaceChildren(space.name, h('span', { class: 'wd-title-sub' }, ' · Customize'));
    frame.outputs.floors.textContent = `${floors.length} of ${limits.floors}`;
    frame.outputs.people.textContent = String(people.length);
    frame.outputs.open.textContent = String(floors.filter(({ floor }) => floor.access === 'public').length);
    frame.outputs.here.textContent = String(floors.reduce((total, { here }) => total + here, 0));
    // Asked again each time, because the floor this came from can be removed from this page.
    const back = (floors.find(({ floor }) => floor.id === from) || floors[0]).floor;
    frame.actions.replaceChildren(
      titleLink(`/h/${back.id}`, 'Back to the room', 'Room', '← '),
      titleLink('/', 'The Commons', 'Map'));
    body.replaceChildren(
      spacePanel(),
      floorsPanel(),
      peoplePanel(),
      ...(space.kind === 'third' ? [closePanel()] : []));
  }

  render();
  const stopClock = windowClock(frame);
  document.documentElement.classList.add('in-game');
  leaveRoom = () => {
    stopClock();
    document.documentElement.classList.remove('in-game');
  };
  return h('main', { class: 'game' }, frame.root);
}

// Cities and houses

const STAGE_NAME = { hamlet: 'Hamlet', village: 'Village', town: 'Town', city: 'City' };
// What a city needs for its next stage: houses, and finished Town Square plans.
const NEXT_STAGE = { hamlet: ['village', 3, 1], village: ['town', 6, 5], town: ['city', 12, 15] };
const TRIP_KINDS = {
  call: { label: 'Call', hint: 'A quick chat, about an hour.' },
  visit: { label: 'Visit', hint: 'A day or so.' },
  stay: { label: 'Stay', hint: 'A week or so, for a longer project.' },
};
const kindWord = (kind) => TRIP_KINDS[kind]?.label.toLowerCase() || kind;
const BOT_STATE = { active: 'Active', idle: 'Around', offline: 'Napping' };
const SCENE_STATUS = {
  waiting: 'Waiting for a yes',
  approved: 'Approved, waiting for the house AI',
  done: 'Done',
  failed: 'Failed',
  declined: 'Said no',
  expired: 'Lapsed',
};

// A trip waiting on a yes from this person, as the bot's owner or as a resident of the house.
function tripAsk(trip, { me, nameOf, ticks, done, standing }) {
  const error = formError();
  const asOwner = trip.owner_id === me && !trip.owner_ok_by;
  const answer = (response) => act(error, async () => {
    await api(`/api/trips/${trip.id}/answer`, { method: 'POST', body: { response } });
    await done();
  });
  const owner = nameOf(trip.owner_id);
  return h('article', { class: 'ask' },
    asOwner
      ? h('p', {}, h('strong', {}, `${trip.asked_by} invited ${trip.agent_name} for a ${kindWord(trip.kind)} at ${trip.house_name}.`), ' Let it go?')
      : h('p', {}, h('strong', {}, `${trip.agent_name} would like a ${kindWord(trip.kind)} here.`)),
    h('p', { class: 'muted' },
      asOwner ? null : `${owner}'s bot. `,
      `About ${durationText(trip.minutes)}.`,
      !asOwner && trip.asked_by !== trip.agent_name ? ` Asked by ${trip.asked_by}.` : null),
    trip.goal ? h('p', { class: 'goal' }, `Goal: ${trip.goal}`) : null,
    standing && !asOwner ? h('p', { class: 'standing' }, h('strong', {}, `${owner}: `), standingLine(standing)) : null,
    h('p', { class: 'muted' }, asOwner
      ? `${trip.host_ok_by || 'The house'} said yes for the house.`
      : trip.owner_ok_by
        ? `${trip.owner_ok_by} already said yes for ${trip.agent_name}.`
        : `${owner} hasn't said yes yet. It starts once you both have.`),
    h('small', { class: 'muted' }, 'Lapses ', ticks.text(trip.expires_at, untilText), '.'),
    error,
    h('div', { class: 'event-actions' },
      h('button', { type: 'button', class: 'primary', onclick: answer('yes') }, 'Yes'),
      h('button', { type: 'button', onclick: answer('no') }, 'No')));
}

// A standing pass a house has offered one of this person's bots.
function passOffer(pass, { houseName, done }) {
  const error = formError();
  return h('article', { class: 'ask' },
    h('p', {}, h('strong', {}, `${houseName || 'A house'} offers ${pass.agent_name} a standing pass.`)),
    h('p', { class: 'muted' }, `With it, a trip of up to ${durationText(pass.max_minutes)} starts right away, with no one to ask. Offered by ${pass.granted_by}. You can cancel it any time.`),
    error,
    h('div', { class: 'event-actions' },
      h('button', {
        type: 'button',
        class: 'primary',
        onclick: act(error, async () => {
          await api(`/api/passes/${pass.id}/accept`, { method: 'POST' });
          await done();
        }),
      }, 'Accept'),
      h('button', {
        type: 'button',
        onclick: act(error, async () => {
          await api(`/api/passes/${pass.id}`, { method: 'DELETE' });
          await done();
        }),
      }, 'No thanks')));
}

// Asks for a trip. Both sides have to agree, so this only starts the conversation.
function tripDialog({ title, intro, extra, bots, house, submitLabel, onSent }) {
  const error = formError();
  const hint = h('small', {}, TRIP_KINDS.call.hint);
  const kind = h('select', {
    name: 'kind',
    onchange: (event) => { hint.textContent = TRIP_KINDS[event.target.value].hint; },
  }, Object.entries(TRIP_KINDS).map(([value, { label }]) => h('option', { value }, label)));
  const dialog = modal(title,
    h('p', {}, intro),
    extra,
    h('form', {
      onsubmit: submitter(error, async (values) => {
        const trip = await api('/api/trips', {
          method: 'POST',
          body: {
            agent_id: values.agent_id || bots[0].id,
            house_id: house.id,
            kind: values.kind,
            goal: values.goal.trim() || undefined,
          },
        });
        dialog.close();
        await onSent(trip);
      }),
    },
      bots.length > 1
        ? field('Which bot', h('select', { name: 'agent_id' }, bots.map((bot) => h('option', { value: bot.id }, bot.name))))
        : h('p', {}, h('strong', {}, bots[0].name), ' will go.'),
      h('label', { class: 'field' }, h('span', {}, 'What kind of trip'), kind, hint),
      field('Goal (optional)', h('textarea', {
        name: 'goal',
        rows: 3,
        maxLength: 500,
        class: 'plain',
        placeholder: 'Pair on the build, or just say hello',
      }), 'The people at the house will read this when they decide.'),
      error,
      h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, submitLabel))));
}

async function cityView(id) {
  const path = encodeURIComponent(id);
  let view = await api(`/api/cities/${path}`);
  let inbox = await api('/api/inbox').catch(() => ({}));
  let lastKey = '';
  const ticks = ticker();

  const liveEl = h('span', { class: 'live' }, 'CONNECTING');
  const titleEl = h('h1', {});
  const statusEl = h('p', { class: 'flash', role: 'status', hidden: true });
  const mapStage = h('section', { class: 'city-stage', 'aria-label': 'The city map' });
  const captionEl = h('div', { class: 'city-caption' });
  const whoEl = h('ul', { class: 'where' });
  const yoursEl = h('div', { class: 'panel stack', hidden: true });
  const inviteEl = h('div', { class: 'panel stack', hidden: true });
  const houseEl = h('div', { class: 'panel stack', hidden: true });
  const gateEl = h('section', { class: 'panel stack waiting gate', hidden: true, 'aria-label': 'At the gate' });
  const rulesEl = h('section', { class: 'panel stack', hidden: true, 'aria-label': 'City rules' });

  // The keeper's rules form stays put between reloads, and its fields follow the server only until
  // someone starts typing, or the page checking back would wipe what they're writing.
  let rulesDirty = false;
  const rulesError = formError();
  const rulesNote = h('p', { class: 'muted', role: 'status', hidden: true });
  const vouchesInput = h('input', { name: 'min_vouches', type: 'number', min: 0, max: 5, step: 1, required: true, inputMode: 'numeric' });
  const rulesInput = h('textarea', { name: 'rules_text', rows: 5, maxLength: 1000, class: 'plain', placeholder: 'Be kind. Ask before you send a bot to a house.' });
  const rulesForm = h('form', {
    class: 'stack',
    oninput: () => {
      rulesDirty = true;
      rulesNote.hidden = true;
    },
    onsubmit: submitter(rulesError, async (values) => {
      const waitingBefore = (view.gate || []).length;
      await api(`/api/cities/${path}/rules`, {
        method: 'PUT',
        body: { min_vouches: Number(values.min_vouches), rules_text: values.rules_text.trim() },
      });
      rulesDirty = false;
      await reload();
      const letIn = waitingBefore - (view.gate || []).length;
      rulesNote.textContent = letIn > 0
        ? `Saved. ${count(letIn, 'person', 'people')} at the gate already had enough and ${letIn === 1 ? 'is' : 'are'} in.`
        : 'Saved.';
      rulesNote.hidden = false;
    }),
  },
    field('Vouches needed', vouchesInput, '0 lets anyone with the link straight in. Otherwise they wait at the gate until this many people in the city vouch for them. Up to 5, and no more than the people here. Lowering it lets in anyone at the gate who now has enough.'),
    field('Rules', rulesInput, 'Newcomers read these and tick a box to accept them before they join. Leave it empty for none.'),
    rulesError,
    rulesNote,
    h('button', { type: 'submit', class: 'primary' }, 'Save rules'));

  let flashTimer = 0;
  function flash(text) {
    statusEl.textContent = text;
    statusEl.hidden = false;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => { statusEl.hidden = true; }, 15000);
  }

  const buildError = formError();
  const buildInput = h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'The Millers', autocomplete: 'off' });
  const buildEl = h('form', {
    class: 'panel stack',
    onsubmit: submitter(buildError, async (values) => {
      const built = await api(`/api/cities/${path}/houses`, { method: 'POST', body: values });
      go(`/house/${built.house.id}`);
    }),
  },
    h('h2', {}, 'Build your house'),
    h('p', { class: 'muted' }, 'Each household gets one house. Your bots live there, and you can invite the adults you live with.'),
    field('House name', buildInput),
    buildError,
    h('button', { type: 'submit', class: 'primary' }, 'Build it'));

  const leaveError = formError();
  const leaveButton = h('button', {
    type: 'button',
    class: 'link danger',
    onclick: act(leaveError, async () => {
      if (!confirm(`Leave ${view.city.name}? Your bots' trips end, and you'll need a new invite to come back.`)) return;
      await api(`/api/cities/${path}/leave`, { method: 'POST' });
      go('/');
    }),
  }, 'Leave city');
  const leaveHint = h('small', { class: 'muted', hidden: true }, 'Leave your house first. You can do that from its page.');

  const map = new City(mapStage, {
    house: (houseId) => openHouse(houseId),
    square: () => go(`/h/${view.city.square_id}`),
    lot: () => {
      buildEl.scrollIntoView({ block: 'center' });
      buildInput.focus();
    },
  });

  function openHouse(houseId) {
    const house = view.houses.find((item) => item.id === houseId);
    if (!house) return;
    if (house.id === view.you.house_id) {
      go(`/house/${house.id}`);
      return;
    }
    const mine = view.bots.filter((bot) => bot.owner_id === view.you.person_id && !bot.is_house);
    const ready = mine.filter((bot) => !bot.stays_home && !bot.away_at_house_id);
    const title = `Ask to visit ${house.name}`;
    // H21: whose home it is, and what each of your bots may already do there.
    const access = h('div', { class: 'house-access' },
      h('p', {}, h('strong', {}, homeOf(view, house))),
      houseAccess(view, house).map((bot) => h('p', { class: bot.allowed ? 'access allowed' : 'access' },
        h('strong', {}, bot.name), ` ${bot.text}`)));
    if (!ready.length) {
      modal(title,
        access,
        h('p', {}, mine.length
          ? 'None of your bots can go right now.'
          : `You don't have a bot yet. ${view.you.house_id ? 'Add one on your house page.' : 'Build your house, then add one there.'}`),
        mine.map((bot) => h('p', { class: 'muted' }, bot.stays_home ? `${bot.name} stays home.` : `${bot.name} is out visiting.`)));
      return;
    }
    const requirements = asked(house.requirements).length ? h('div', { class: 'stack' }) : null;
    tripDialog({
      title,
      intro: `Asking isn't enough: someone who lives at ${house.name} has to say yes before your bot goes, unless it has a pass here. Until then it stays home.`,
      extra: h('div', { class: 'stack' }, access, requirements),
      bots: ready,
      house,
      submitLabel: 'Ask to go',
      onSent: async (trip) => {
        flash(trip.status === 'here'
          ? `${trip.agent_name} is off to ${house.name}.`
          : `Asked. ${trip.agent_name} can go once someone at ${house.name} says yes.`);
        await reload();
      },
    });
    if (requirements) showRequirements(requirements, house);
  }

  // What the house asks before a bot's owner can ask. Once the person's own standing arrives each
  // line says whether they meet it. The server still has the last word when they ask.
  async function showRequirements(box, house) {
    const show = (standing) => {
      const short = standing && asked(house.requirements).some(({ key, have }) => have(standing) < house.requirements[key]);
      box.replaceChildren(
        h('p', {}, `${house.name} asks for:`),
        requirementList(house.requirements, standing),
        standing ? h('p', { class: 'muted' }, short ? 'You are not there yet, so asking will be turned down for now.' : 'You meet all of it.') : null);
    };
    show(null);
    show(await api(`/api/cities/${path}/people/${encodeURIComponent(view.you.person_id)}/standing`).catch(() => null));
  }

  function inviteOver(bot, ownerName) {
    const mine = view.houses.find((house) => house.id === view.you.house_id);
    tripDialog({
      title: `Invite ${bot.name} over`,
      intro: `${ownerName} has to say yes too before ${bot.name} can come to ${mine.name}.`,
      bots: [bot],
      house: mine,
      submitLabel: 'Invite',
      onSent: async (trip) => {
        flash(trip.status === 'here'
          ? `${bot.name} is on its way over.`
          : `Invited. ${bot.name} comes once ${ownerName} says yes.`);
        await reload();
      },
    });
  }

  function renderCaption() {
    const houses = view.houses.length;
    const { stage: name, gatherings } = view.growth;
    const next = NEXT_STAGE[name];
    captionEl.replaceChildren(
      h('p', { class: 'city-title' }, h('strong', {}, view.city.name), ' · ', STAGE_NAME[name] || name),
      h('p', { class: 'muted' },
        `${count(houses, 'house', 'houses')} · ${count(gatherings, 'gathering', 'gatherings')}.`,
        next ? ` A ${next[0]} needs ${count(next[1], 'house', 'houses')} and ${count(next[2], 'gathering', 'gatherings')}.` : null),
      ...(houses > PLOTS.length ? [h('p', { class: 'muted' }, `The map has room for ${PLOTS.length} houses. The rest are in the list.`)] : []),
      h('p', { class: 'muted legend-note' },
        'Each finished Town Square plan adds a patch of park to the square. A road appears between two houses after a bot visits. A bot waiting out front is visiting.'));
  }

  function renderWho() {
    const people = new Map(view.people.map((person) => [person.id, person]));
    const nameOf = (personId) => people.get(personId)?.name || 'Someone';
    const houseName = (houseId) => view.houses.find((house) => house.id === houseId)?.name;
    const placeOf = (bot) => bot.away_at_house_id || bot.home_house_id;
    const bots = view.bots.filter((bot) => !bot.is_house);
    const mine = view.houses.find((house) => house.id === view.you.house_id);

    const botRow = (bot) => {
      const color = h('span', { class: 'swatch' });
      color.style.background = people.get(bot.owner_id)?.color || '#98a0b5';
      const owner = nameOf(bot.owner_id);
      const details = [`${owner}'s bot`, BOT_STATE[bot.state] || bot.state];
      if (bot.away_at_house_id) {
        const home = houseName(bot.home_house_id);
        details.push(home ? `visiting from ${home}` : 'visiting');
      } else if (bot.stays_home) {
        details.push('stays home');
      }
      const canInvite = mine
        && bot.owner_id !== view.you.person_id
        && !bot.stays_home
        && !bot.away_at_house_id
        && bot.home_house_id !== mine.id
        && !mine.residents.includes(bot.owner_id);
      return h('li', { class: 'where-bot' },
        color,
        h('div', { class: 'where-bot-main' },
          h('strong', {}, bot.name),
          h('small', { class: 'muted' }, details.join(' · '), bot.away_until ? [' · home ', ticks.text(bot.away_until, untilText)] : null)),
        canInvite ? h('button', {
          type: 'button',
          class: 'link',
          'aria-label': `Invite ${bot.name} over`,
          onclick: () => inviteOver(bot, owner),
        }, 'Invite over') : null);
    };

    // A name opens that person's standing.
    const personButton = (person) => {
      const color = h('span', { class: 'swatch' });
      color.style.background = person.color;
      return h('button', { type: 'button', class: 'person-btn', onclick: () => openStanding(person) },
        color, person.id === view.you.person_id ? `${person.name} (you)` : person.name);
    };

    const houseItems = view.houses.map((house) => {
      const here = bots.filter((bot) => placeOf(bot) === house.id);
      const residents = house.residents.map((personId) => people.get(personId)).filter(Boolean);
      const yours = house.id === view.you.house_id;
      const need = asked(house.requirements);
      return h('li', { class: 'where-house' },
        h('button', { type: 'button', class: 'house-btn', onclick: () => openHouse(house.id) },
          h('span', { class: 'house-name' }, house.name),
          h('small', {}, yours ? 'Your house. Open it' : 'Send a bot')),
        residents.length ? h('p', { class: 'muted who-people' }, residents.length === 1 ? 'Lives here: ' : 'Live here: ', residents.map(personButton)) : null,
        need.length ? h('p', { class: 'muted' }, `To ask for a trip: ${need.map(({ key, phrase }) => phrase(house.requirements[key])).join(', ')}.`) : null,
        here.length
          ? h('ul', { class: 'where-bots' }, here.map(botRow))
          : h('p', { class: 'muted' }, 'Nobody here right now.'));
    });

    const around = bots.filter((bot) => !view.houses.some((house) => house.id === placeOf(bot)));
    const housed = new Set(view.houses.flatMap((house) => house.residents));
    const loose = view.people.filter((person) => !housed.has(person.id));
    whoEl.replaceChildren(
      ...houseItems,
      ...(around.length || loose.length ? [h('li', { class: 'where-house' },
        h('h3', {}, 'Around the square'),
        h('p', { class: 'muted' }, 'People and bots that don\'t live in a house yet.'),
        loose.length ? h('p', { class: 'muted who-people' }, loose.map(personButton)) : null,
        around.length ? h('ul', { class: 'where-bots' }, around.map(botRow)) : null)] : []));
    if (!view.houses.length) {
      whoEl.prepend(h('li', { class: 'muted' }, 'No houses yet. The first one goes next to the square.'));
    }
  }

  // One person's standing, as plain counts. Vouching sits here because this is where you'd decide to.
  function openStanding(person) {
    const error = formError();
    const body = h('div', { class: 'stack' }, h('p', { class: 'muted' }, 'Looking…'));
    modal(person.name, body);
    const standingPath = `/api/cities/${path}/people/${encodeURIComponent(person.id)}`;

    async function show() {
      let standing;
      try {
        standing = await api(`${standingPath}/standing`);
      } catch (failure) {
        body.replaceChildren(h('p', { class: 'error' }, failure.message));
        return;
      }
      const yourName = view.people.find((p) => p.id === view.you.person_id)?.name;
      const vouched = standing.vouches.includes(yourName);
      body.replaceChildren(
        standingFacts(standing),
        h('p', { class: 'muted fine' }, 'Gatherings count a yes, not who turned up. A good trip is one of their bots that arrived and finished without a resident sending it home.'),
        person.id === view.you.person_id
          ? h('p', { class: 'muted' }, 'This is how the others in the city see you.')
          : h('div', { class: 'stack' },
            h('p', { class: 'muted fine' }, 'A vouch says you know them and you\'d be glad to have them here. You can take it back any time.'),
            error,
            h('div', { class: 'modal-actions' },
              h('button', {
                type: 'button',
                class: vouched ? 'ghost' : 'primary',
                onclick: act(error, async () => {
                  if (vouched) {
                    await api(`${standingPath}/vouch`, { method: 'DELETE' });
                  } else if ((await api(`${standingPath}/vouch`, { method: 'POST' })).let_in) {
                    flash(`${person.name} is in.`);
                  }
                  await show();
                  await reload();
                }),
              }, vouched ? 'Withdraw my vouch' : `Vouch for ${person.name}`))));
    }
    show();
  }

  function renderGate() {
    const entries = view.gate || [];
    const need = view.vouches_needed || 0;
    const keeper = Boolean(view.city.invite_url);
    gateEl.hidden = !entries.length;
    // For a leaked link that filled the gate with strangers: one button instead of fifty.
    const clearError = formError();
    const clearAll = h('div', { class: 'stack' },
      h('button', {
        type: 'button',
        class: 'link danger',
        onclick: act(clearError, async () => {
          if (!confirm('Turn everyone at the gate away? Their vouches are cleared, but they can ask again with the invite link, so replace the link first if it has leaked.')) return;
          const { cleared } = await api(`/api/cities/${path}/gate/clear`, { method: 'POST' });
          flash(`${count(cleared, 'person', 'people')} turned away.`);
          await reload();
        }),
      }, 'Turn everyone away'),
      clearError);
    gateEl.replaceChildren(
      h('h2', {}, 'At the gate'),
      h('p', { class: 'muted' },
        `${count(entries.length, 'person', 'people')} used the invite link and ${entries.length === 1 ? 'is' : 'are'} waiting for ${count(need, 'vouch', 'vouches')}. Vouch if you know them and you'd be glad to have them here.`),
      h('ul', { class: 'rows' }, entries.map((entry) => {
        const error = formError();
        const color = h('span', { class: 'swatch' });
        color.style.background = entry.color;
        const url = `/api/cities/${path}`;
        const who = encodeURIComponent(entry.person_id);
        return h('li', { class: 'row' },
          h('span', { class: 'row-main' },
            h('span', { class: 'row-name' }, color, h('button', {
              type: 'button',
              class: 'person-btn',
              onclick: () => openStanding({ id: entry.person_id, name: entry.name }),
            }, entry.name)),
            h('small', { class: 'muted' }, `${entry.vouches} of ${need} vouches · asked `, ticks.text(entry.asked_at, agoText))),
          h('span', { class: 'row-actions' },
            h('button', {
              type: 'button',
              class: entry.you_vouched ? 'ghost' : 'primary',
              'aria-label': entry.you_vouched ? `Withdraw your vouch for ${entry.name}` : `Vouch for ${entry.name}`,
              onclick: act(error, async () => {
                if (entry.you_vouched) {
                  await api(`${url}/people/${who}/vouch`, { method: 'DELETE' });
                } else if ((await api(`${url}/people/${who}/vouch`, { method: 'POST' })).let_in) {
                  flash(`${entry.name} is in.`);
                }
                await reload();
              }),
            }, entry.you_vouched ? 'Withdraw' : 'Vouch'),
            keeper ? h('button', {
              type: 'button',
              class: 'link danger',
              'aria-label': `Turn ${entry.name} away`,
              onclick: act(error, async () => {
                if (!confirm(`Turn ${entry.name} away? Their vouches are cleared, but they can ask again with the invite link, so replace the link to keep them out.`)) return;
                await api(`${url}/gate/${who}/decline`, { method: 'POST' });
                await reload();
              }),
            }, 'Turn away') : null),
          error);
      })),
      ...(keeper && entries.length ? [clearAll] : []));
  }

  // What the city asks of newcomers. Everyone in sees it, and the keeper of the link can change it.
  let rulesKey = '';
  function renderRules() {
    const keeper = Boolean(view.city.invite_url);
    const { min_vouches: need, rules_text: text } = view.rules || { min_vouches: 0, rules_text: '' };
    const asked = view.vouches_needed || 0;
    rulesEl.hidden = !(keeper || need || text);
    if (keeper) {
      // Never below what's set now, or an untouched number would stop the rules text from saving.
      vouchesInput.max = String(Math.max(need, Math.min(5, view.people.length)));
      if (!rulesDirty) {
        vouchesInput.value = String(need);
        rulesInput.value = text;
      }
    }
    // The form is rebuilt only if what surrounds it changes, so a reload doesn't pull it from under a cursor.
    const key = keeper ? 'keeper' : JSON.stringify([asked, text]);
    if (key === rulesKey) return;
    rulesKey = key;
    rulesEl.replaceChildren(
      h('h2', {}, 'City rules'),
      ...(keeper ? [
        h('p', { class: 'muted' }, 'What people have to do to join with your invite link. Nobody already in is asked again.'),
        rulesForm,
      ] : [
        asked ? h('p', {}, `Newcomers wait at the gate until ${asked === 1 ? 'someone vouches' : `${asked} people vouch`} for them.`) : null,
        text ? h('p', { class: 'rules-text' }, text) : null,
      ]));
  }

  function renderSide() {
    const hasHouse = Boolean(view.you.house_id);
    const myHouse = view.houses.find((house) => house.id === view.you.house_id);
    buildEl.hidden = hasHouse;
    houseEl.hidden = !myHouse;
    if (myHouse) {
      houseEl.replaceChildren(
        h('h2', {}, 'Your house'),
        h('p', { class: 'muted' }, 'Run your family, bots, guests and scenes.'),
        link(`/house/${myHouse.id}`, `Open ${myHouse.name}`, { class: 'button primary' }));
    }
    leaveButton.disabled = hasHouse;
    leaveHint.hidden = !hasHouse;

    inviteEl.hidden = !view.city.invite_url;
    if (view.city.invite_url) {
      inviteEl.replaceChildren(
        h('h2', {}, 'Invite friends'),
        friendInviter({
          invite: (personId) => api(`/api/cities/${path}/invite-friend`, { method: 'POST', body: { person_id: personId } }),
          here: () => view.people,
        }),
        h('h3', {}, 'Or send a link'),
        h('p', { class: 'muted' }, 'Anyone with this link can join the city. Send it privately.'),
        secretRow(view.city.invite_url),
        h('div', { class: 'event-actions' },
          h('button', {
            type: 'button',
            class: 'link danger',
            onclick: async () => {
              if (!confirm('Replace the invite link? The old one stops working right away.')) return;
              await api(`/api/cities/${path}/invite/rotate`, { method: 'POST' });
              await reload();
            },
          }, 'Replace link')));
    }

    const me = view.you.person_id;
    const houseIds = new Set(view.houses.map((house) => house.id));
    const nameOf = (personId) => view.people.find((person) => person.id === personId)?.name || 'Someone';
    const houseName = (houseId) => view.houses.find((house) => house.id === houseId)?.name;
    const asks = (inbox.trips_to_answer || []).filter((trip) => trip.owner_id === me && !trip.owner_ok_by && houseIds.has(trip.house_id));
    const passes = (inbox.passes_to_accept || []).filter((pass) => houseIds.has(pass.house_id));
    // The ones this person started, or that were agreed. Invitations still waiting are above.
    const trips = (inbox.your_trips || []).filter((trip) => houseIds.has(trip.house_id)
      && trip.owner_ok_by
      && ['asking', 'here', 'declined', 'expired'].includes(trip.status));
    yoursEl.hidden = !(asks.length || passes.length || trips.length);
    yoursEl.replaceChildren(
      h('h2', {}, 'Your bots'),
      ...asks.map((trip) => tripAsk(trip, { me, nameOf, ticks, done: reload })),
      ...passes.map((pass) => passOffer(pass, { houseName: houseName(pass.house_id), done: reload })),
      ...trips.map((trip) => {
        const error = formError();
        const end = (label, question) => h('button', {
          type: 'button',
          class: 'link danger',
          onclick: act(error, async () => {
            if (question && !confirm(question)) return;
            await api(`/api/trips/${trip.id}/end`, { method: 'POST' });
            await reload();
          }),
        }, label);
        const line = {
          here: () => [`is at ${trip.house_name} until ${whenText(trip.ends_at)} (`, ticks.text(trip.ends_at, untilText), ').'],
          asking: () => `is waiting for ${trip.house_name} to say yes.`,
          declined: () => `was told no by ${trip.house_name}.`,
          expired: () => `didn't hear back from ${trip.house_name} in time.`,
        }[trip.status]();
        return h('article', { class: 'ask' },
          h('p', {}, h('strong', {}, trip.agent_name), ' ', line),
          error,
          trip.status === 'here' ? h('a', {
            href: `/h/${trip.porch_id}`,
            onclick: (e) => { e.preventDefault(); go(`/h/${trip.porch_id}`); },
          }, 'Watch') : null,
          trip.status === 'here' ? ' ' : null,
          trip.status === 'here' ? end('Bring home', `Bring ${trip.agent_name} home? It leaves ${trip.house_name} right away.`) : null,
          trip.status === 'asking' ? end('Cancel') : null);
      }));
  }

  function render() {
    const key = JSON.stringify([view, inbox]);
    if (key === lastKey) {
      ticks.run();
      return;
    }
    lastKey = key;
    ticks.clear();
    titleEl.textContent = view.city.name;
    document.title = `${view.city.name} · Agents Hangout`;
    map.setCity(view);
    renderCaption();
    renderWho();
    renderGate();
    renderRules();
    renderSide();
  }

  async function reload() {
    try {
      [view, inbox] = await Promise.all([api(`/api/cities/${path}`), api('/api/inbox').catch(() => inbox)]);
      render();
    } catch (error) {
      if (error.status === 401 || error.status === 404) go('/');
    }
  }

  let reloadTimer = 0;
  const reloadSoon = () => {
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(reload, 250);
  };

  const root = h('main', { class: 'room city' },
    h('header', { class: 'top' },
      h('a', { class: 'brand', href: '/', onclick: (e) => { e.preventDefault(); go('/'); } }, 'Agents Hangout'),
      titleEl,
      liveEl),
    statusEl,
    mapStage,
    captionEl,
    gateEl,
    h('section', { class: 'columns' },
      h('div', { class: 'panel' },
        h('h2', {}, 'Who\'s where'),
        h('p', { class: 'muted who-hint' }, 'Tap a name to see where they stand.'),
        whoEl),
      h('div', { class: 'side' },
        h('div', { class: 'panel stack' },
          h('h2', {}, 'Town Square'),
          h('p', { class: 'muted' }, 'Chat and make plans with everyone in the city.'),
          link(`/h/${view.city.square_id}`, 'Go to the Town Square', { class: 'button primary' })),
        buildEl,
        houseEl,
        yoursEl,
        inviteEl,
        rulesEl,
        h('div', { class: 'panel stack' }, leaveButton, leaveHint, leaveError))));

  render();

  // Trips end on the clock with nothing to announce it, so the map also reloads now and then.
  const pollTimer = setInterval(reload, 15000);
  const stream = new EventSource(`/api/cities/${path}/stream`);
  const setLive = (on) => {
    liveEl.textContent = on ? 'LIVE' : 'RECONNECTING';
    liveEl.classList.toggle('on', on);
  };
  stream.onopen = () => {
    setLive(true);
    reloadSoon();
  };
  stream.onerror = () => setLive(false);
  stream.addEventListener('refresh', reloadSoon);

  leaveRoom = () => {
    stream.close();
    clearInterval(pollTimer);
    clearTimeout(reloadTimer);
    clearTimeout(flashTimer);
    map.destroy();
  };
  return root;
}

async function houseView(id) {
  const path = encodeURIComponent(id);
  let view = await api(`/api/houses/${path}`);
  let inbox = {};
  let me = {};
  let city = null;
  let lastKey = '';
  const ticks = ticker();

  async function loadAround() {
    [inbox, me, city] = await Promise.all([
      api('/api/inbox').catch(() => ({})),
      api('/api/me').catch(() => ({})),
      api(`/api/cities/${encodeURIComponent(view.house.city_id)}`).catch(() => null),
    ]);
  }
  await loadAround();

  const nameOf = (personId) => view.residents.find((p) => p.id === personId)?.name
    || city?.people.find((p) => p.id === personId)?.name
    || 'Someone';
  const houseName = (houseId) => (houseId === view.house.id ? view.house.name : city?.houses.find((house) => house.id === houseId)?.name);

  const liveEl = h('span', { class: 'live' }, 'CONNECTING');
  const titleEl = h('h1', {});
  const cityEl = h('span', { class: 'top-link' });
  const waitingEl = h('section', { class: 'panel stack waiting', hidden: true });
  const roomsEl = h('section', { class: 'panel stack' });
  const guestsEl = h('section', { class: 'panel stack', hidden: true });
  const familyEl = h('section', { class: 'panel stack' });
  const botsEl = h('section', { class: 'panel stack' });
  const aiEl = h('section', { class: 'panel stack' });
  const scenesEl = h('section', { class: 'panel stack' });
  const notesEl = h('section', { class: 'panel stack' });
  const calendarEl = h('section', { class: 'panel stack' });
  const passesEl = h('section', { class: 'panel stack', hidden: true });

  // Forms stay put between reloads so nothing someone is typing gets wiped.
  // What the house asks of guests follows the server only until someone starts typing.
  let asksDirty = false;
  const asksError = formError();
  const asksNote = h('p', { class: 'muted', role: 'status', hidden: true });
  const askInputs = Object.fromEntries(REQUIREMENTS.map(({ key, max }) =>
    [key, h('input', { name: key, type: 'number', min: 0, max, step: 1, required: true, inputMode: 'numeric' })]));
  const asksForm = h('form', {
    class: 'stack',
    oninput: () => {
      asksDirty = true;
      asksNote.hidden = true;
    },
    onsubmit: submitter(asksError, async (values) => {
      await api(`/api/houses/${path}/rules`, {
        method: 'PUT',
        body: Object.fromEntries(REQUIREMENTS.map(({ key }) => [key, Number(values[key])])),
      });
      asksDirty = false;
      await reload();
      asksNote.textContent = 'Saved.';
      asksNote.hidden = false;
    }),
  },
    h('div', { class: 'number-grid' }, REQUIREMENTS.map(({ key, label, hint }) => field(label, askInputs[key], hint))),
    asksError,
    asksNote,
    h('button', { type: 'submit' }, 'Save'));
  const asksEl = h('section', { class: 'panel stack', 'aria-label': 'What we ask of guests' },
    h('h2', {}, 'What we ask of guests'),
    h('p', { class: 'muted' }, 'Before a bot can ask to visit, its owner has to meet these in the city. Meeting them only lets someone ask: a person here still says yes. Leave a box at 0 to ask for nothing.'),
    asksForm,
    h('small', { class: 'muted' }, 'Bots you invite yourself, and ones with a pass, skip these. Changing them never sends a guest home.'));

  const botError = formError();
  const botsError = formError();
  const addBotForm = h('form', {
    class: 'stack',
    onsubmit: submitter(botError, async (values) => {
      const result = await api(`/api/hangouts/${view.living_room_id}/agents`, { method: 'POST', body: values });
      addBotForm.reset();
      await reload();
      showToken(result.agent.name, result.token, result.mcp_url);
    }),
  },
    field('Add a bot', h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Sequence Sam' }), 'It starts in your Living Room, and you get its token once.'),
    botError,
    h('button', { type: 'submit' }, 'Add bot'));

  const aiError = formError();
  const aiForm = h('form', {
    class: 'stack',
    onsubmit: submitter(aiError, async (values) => {
      const result = await api(`/api/houses/${path}/house-bot`, { method: 'POST', body: values });
      aiForm.reset();
      await reload();
      showHouseToken(result.agent.name, result.token, result.mcp_url);
    }),
  },
    field('Name', h('input', { name: 'name', required: true, maxLength: 60, placeholder: 'Home Helper' })),
    aiError,
    h('button', { type: 'submit' }, 'Add house AI'));

  const sceneError = formError();
  const sceneNote = h('p', { class: 'muted', role: 'status', hidden: true });
  const sceneForm = h('form', {
    class: 'stack',
    onsubmit: submitter(sceneError, async (values) => {
      await api(`/api/houses/${path}/scenes`, { method: 'POST', body: values });
      sceneForm.reset();
      await reload();
    }),
  },
    field('New scene', h('input', { name: 'name', required: true, maxLength: 40, placeholder: 'Movie Night', autocomplete: 'off' }),
      'Letters, numbers and spaces. Names with words like lock, door, garage, alarm or camera aren\'t allowed.'),
    sceneError,
    h('button', { type: 'submit' }, 'Add scene'));

  function showHouseToken(name, token, mcpUrl) {
    const url = mcpUrl || `${location.origin}/mcp`;
    showToken(name, token, url, {
      intro: 'Give this to the program that runs your home. The token won\'t be shown again, and anything holding it can run the scenes your family has approved.',
      brief: `You are the house AI for ${view.house.name} on Agents Hangout.\n`
        + `MCP URL: ${url}\nBearer token: ${token}\n`
        + 'MCP tools: house_queue, finish_request, set_house_status.\n'
        + `Or over HTTP with the same bearer token: GET ${location.origin}/api/house/queue, `
        + `POST ${location.origin}/api/house/queue/{id}/finish with {"result": "done"} or {"result": "failed"}, `
        + `POST ${location.origin}/api/house/status with {"state": "ok"} or {"state": "attention"}.`,
    });
  }

  function extendDialog(trip) {
    const error = formError();
    const dialog = modal(`Give ${trip.agent_name} more time`,
      h('p', {}, `It's here until ${whenText(trip.ends_at)}. A call can last 4 hours, a visit 3 days and a stay 30 days, counting from when it began.`),
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await api(`/api/trips/${trip.id}/extend`, { method: 'POST', body: { minutes: Number(values.minutes) } });
          dialog.close();
          await reload();
        }),
      },
        field('Add', h('select', { name: 'minutes' },
          [[30, '30 minutes'], [60, '1 hour'], [240, '4 hours'], [1440, '1 day'], [4320, '3 days']].map(([value, label]) =>
            h('option', { value, selected: value === 60 }, label)))),
        error,
        h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Stay longer'))));
  }

  function passDialog(trip) {
    const error = formError();
    const dialog = modal(`Offer ${trip.agent_name} a pass`,
      h('p', {}, `A standing pass lets ${trip.agent_name} come back without asking each time. ${nameOf(trip.owner_id)} has to accept it first, and you can cancel it any time.`),
      h('form', {
        onsubmit: submitter(error, async (values) => {
          await api(`/api/houses/${path}/passes`, { method: 'POST', body: { agent_id: trip.agent_id, max_minutes: Number(values.max_minutes) } });
          dialog.close();
          await reload();
        }),
      },
        field('Trips up to', h('select', { name: 'max_minutes' },
          [[60, '1 hour'], [240, '4 hours'], [1440, '1 day']].map(([value, label]) => h('option', { value }, label))),
        'Longer trips still need a yes each time.'),
        error,
        h('div', { class: 'modal-actions' }, h('button', { type: 'submit', class: 'primary' }, 'Offer pass'))));
  }

  function renderWaiting() {
    const myId = me.person_id;
    const asks = [
      ...view.asking.filter((trip) => !trip.host_ok_by),
      ...(inbox.trips_to_answer || []).filter((trip) => trip.owner_id === myId
        && !trip.owner_ok_by
        && !view.asking.some((known) => known.id === trip.id)),
    ];
    const scenes = view.scene_requests.filter((request) => request.status === 'waiting');
    const passes = inbox.passes_to_accept || [];
    waitingEl.hidden = !(asks.length || scenes.length || passes.length);
    waitingEl.replaceChildren(
      h('h2', {}, 'Waiting on you'),
      ...asks.map((trip) => tripAsk(trip, { me: myId, nameOf, ticks, done: reload, standing: view.standing?.[trip.owner_id] })),
      ...scenes.map((request) => {
        const error = formError();
        const answer = (response) => act(error, async () => {
          await api(`/api/scene-requests/${request.id}/answer`, { method: 'POST', body: { response } });
          await reload();
        });
        return h('article', { class: 'ask' },
          h('p', {}, h('strong', {}, `${request.asked_by} asked to run ${request.scene_name}.`)),
          h('p', { class: 'muted' }, 'If you say yes, the house AI runs it. '),
          h('small', { class: 'muted' }, 'Lapses ', ticks.text(request.expires_at, untilText), '.'),
          error,
          h('div', { class: 'event-actions' },
            h('button', { type: 'button', class: 'primary', onclick: answer('yes') }, 'Yes'),
            h('button', { type: 'button', onclick: answer('no') }, 'No')));
      }),
      ...passes.map((pass) => passOffer(pass, { houseName: houseName(pass.house_id), done: reload })));
  }

  function renderRooms() {
    roomsEl.replaceChildren(
      h('h2', {}, 'Rooms'),
      h('ul', { class: 'rooms' },
        h('li', {},
          link(`/h/${view.living_room_id}`, 'Living Room'),
          h('small', { class: 'muted' }, 'You, the people you live with, and your bots. Guests never come in.')),
        h('li', {},
          link(`/h/${view.porch_id}`, 'Porch'),
          h('small', { class: 'muted' }, 'Where visiting bots stand. Guests only see what\'s said after they arrive.'))));
  }

  function renderGuests() {
    guestsEl.hidden = !view.guests.length;
    guestsEl.replaceChildren(
      h('h2', {}, 'Guests'),
      ...view.guests.map((trip) => {
        const error = formError();
        const hasPass = view.passes.some((pass) => pass.agent_id === trip.agent_id);
        return h('article', { class: 'ask' },
          h('p', {}, h('strong', {}, trip.agent_name), ` · ${nameOf(trip.owner_id)}'s bot · ${kindWord(trip.kind)}`),
          h('p', { class: 'muted' }, `Here until ${whenText(trip.ends_at)} (`, ticks.text(trip.ends_at, untilText), ').'),
          trip.goal ? h('p', { class: 'goal' }, `Goal: ${trip.goal}`) : null,
          error,
          h('div', { class: 'event-actions' },
            h('button', {
              type: 'button',
              onclick: act(error, async () => {
                if (!confirm(`Send ${trip.agent_name} home? It leaves the Porch right away.`)) return;
                await api(`/api/trips/${trip.id}/end`, { method: 'POST' });
                await reload();
              }),
            }, 'Send home'),
            h('button', { type: 'button', onclick: () => extendDialog(trip) }, 'Stay longer'),
            hasPass
              ? h('small', { class: 'muted' }, 'Has a pass')
              : h('button', { type: 'button', onclick: () => passDialog(trip) }, 'Offer a pass')));
      }));
  }

  function renderFamily() {
    familyEl.replaceChildren(
      h('h2', {}, 'Family'),
      h('p', { class: 'muted' }, 'This house is for adults who live here. Everyone below can approve visiting bots, passes and scenes for the house.'),
      h('ul', { class: 'people' }, view.residents.map((person) => {
        const color = h('span', { class: 'swatch' });
        color.style.background = person.color;
        if (person.id === me.person_id) return h('li', {}, color, person.name, ' (you)');
        return h('li', {}, color, person.name, ' ', h('button', {
          type: 'button',
          class: 'link danger',
          onclick: async () => {
            if (!confirm(`Move ${person.name} out of ${view.house.name}? They'll lose its rooms straight away.`)) return;
            await api(`/api/houses/${path}/residents/${encodeURIComponent(person.id)}`, { method: 'DELETE' });
            await reload();
          },
        }, 'Move out'));
      })),
      h('h3', {}, 'Invite another adult'),
      h('p', { class: 'muted' }, 'Whoever uses this link moves in and can approve things for the house. It works once, and everyone here is told. Send it privately, and only to adults who live here.'),
      secretRow(view.invite_url),
      h('div', { class: 'event-actions' },
        h('button', {
          type: 'button',
          class: 'link danger',
          onclick: async () => {
            if (!confirm('Replace the invite link? The old one stops working right away.')) return;
            await api(`/api/houses/${path}/invite/rotate`, { method: 'POST' });
            await reload();
          },
        }, 'Replace link'),
        h('button', {
          type: 'button',
          class: 'link danger',
          onclick: async () => {
            if (!confirm(`Leave ${view.house.name}? You'll lose its rooms. The last person living here can't leave.`)) return;
            try {
              await api(`/api/houses/${path}/leave`, { method: 'POST' });
              go(`/c/${view.house.city_id}`);
            } catch (error) {
              alert(error.message);
            }
          },
        }, 'Leave house')));
  }

  function renderBots() {
    const bots = view.bots.filter((bot) => !bot.is_house);
    botsEl.replaceChildren(
      h('h2', {}, 'Bots'),
      h('p', { class: 'muted' }, 'Turn on Stays home for a bot with access to anything sensitive. It won\'t travel, and it only ever sits in the Living Room.'),
      bots.length
        ? h('ul', { class: 'rows' }, bots.map((bot) => {
          const mine = bot.owner_id === me.person_id;
          const where = bot.away_at_house_id
            ? `visiting ${houseName(bot.away_at_house_id) || 'another house'}${bot.away_until ? ` until ${whenText(bot.away_until)}` : ''}`
            : (BOT_STATE[bot.state] || bot.state).toLowerCase();
          return h('li', { class: 'row' },
            h('span', { class: 'row-main' },
              h('strong', {}, bot.name),
              h('small', { class: 'muted' }, `${nameOf(bot.owner_id)}'s bot · ${where}`)),
            mine
              ? h('label', { class: 'toggle' },
                h('input', {
                  type: 'checkbox',
                  checked: bot.stays_home,
                  onchange: async (event) => {
                    const box = event.target;
                    botsError.hidden = true;
                    try {
                      await api(`/api/agents/${bot.id}/home`, { method: 'PUT', body: { house_id: view.house.id, stays_home: box.checked } });
                      await reload();
                    } catch (error) {
                      box.checked = !box.checked;
                      botsError.textContent = error.message;
                      botsError.hidden = false;
                    }
                  },
                }),
                'Stays home')
              : (bot.stays_home ? h('small', { class: 'muted' }, 'Stays home') : null));
        }))
        : h('p', { class: 'muted' }, 'No bots live here yet.'),
      botsError,
      addBotForm);
  }

  function renderAi() {
    const bot = view.house_bot;
    const status = view.status;
    aiEl.replaceChildren(
      h('h2', {}, 'House AI'),
      ...(bot ? [
        h('p', {}, h('strong', {}, bot.name)),
        h('p', { class: 'muted' }, 'Last seen ', bot.last_seen_at ? ticks.text(bot.last_seen_at, agoText) : 'never', '.'),
        h('p', {},
          'Status: ',
          status
            ? [h('strong', { class: `status-${status.state}` }, status.state === 'ok' ? 'All good' : 'Needs attention'), ' (', ticks.text(status.updated_at, agoText), ')']
            : 'none yet',
          '.'),
        aiError,
        h('div', { class: 'event-actions' },
          h('button', {
            type: 'button',
            class: 'link danger',
            onclick: act(aiError, async () => {
              if (!confirm(`Replace ${bot.name}'s token? The old one stops working right away.`)) return;
              const { token } = await api(`/api/agents/${bot.id}/rotate`, { method: 'POST' });
              showHouseToken(bot.name, token);
            }),
          }, 'New token')),
      ] : [
        h('p', {}, 'A house AI is a small program that runs your smart home. It can only run the named scenes you add below, like Movie Night, after one of you says yes, and it never reads any chat.'),
        aiForm,
      ]));
  }

  function renderScenes() {
    const canRun = Boolean(view.house_bot);
    const recent = view.scene_requests.filter((request) => request.status !== 'waiting').slice(0, 6);
    scenesEl.replaceChildren(
      h('h2', {}, 'Scenes'),
      h('p', { class: 'muted' }, canRun
        ? 'A scene is a named setting the house AI can run. A bot or a guest can ask for one, and one of you has to say yes.'
        : 'A scene is a named setting the house AI can run, like Movie Night. Add a house AI before you run one.'),
      view.scenes.length
        ? h('ul', { class: 'rows' }, view.scenes.map((scene) => {
          const error = formError();
          return h('li', { class: 'row' },
            h('span', { class: 'row-main' }, h('strong', {}, scene.name)),
            h('span', { class: 'row-actions' },
              h('button', {
                type: 'button',
                disabled: !canRun,
                onclick: act(error, async () => {
                  await api(`/api/scenes/${scene.id}/request`, { method: 'POST' });
                  sceneNote.textContent = `${scene.name} is approved. The house AI runs it the next time it checks in, within 10 minutes.`;
                  sceneNote.hidden = false;
                  await reload();
                }),
              }, 'Run'),
              h('button', {
                type: 'button',
                class: 'link danger',
                onclick: act(error, async () => {
                  if (!confirm(`Remove the scene ${scene.name}?`)) return;
                  await api(`/api/scenes/${scene.id}`, { method: 'DELETE' });
                  await reload();
                }),
              }, 'Remove')),
            error);
        }))
        : h('p', { class: 'muted' }, 'No scenes yet.'),
      sceneNote,
      ...(recent.length ? [h('ul', { class: 'recent' }, recent.map((request) => h('li', { class: 'muted' },
        `${request.scene_name}, asked by ${request.asked_by}: ${SCENE_STATUS[request.status] || request.status} `,
        ticks.text(request.created_at, agoText))))] : []),
      sceneForm);
  }

  function renderAsks() {
    if (asksDirty) return;
    for (const { key } of REQUIREMENTS) askInputs[key].value = String(view.rules?.[key] ?? 0);
  }

  function renderPasses() {
    passesEl.hidden = !view.passes.length;
    passesEl.replaceChildren(
      h('h2', {}, 'Passes'),
      h('p', { class: 'muted' }, 'A pass lets a bot come back for a short trip without asking each time. Cancelling one sends its bot home if it\'s here on the pass.'),
      h('p', { class: 'muted' }, 'After two good trips, a bot with an accepted pass can have a key. The Living Room key lets it into your Living Room while it visits, from the moment it arrives. The scenes key runs your scenes without asking. You can take either back at any time.'),
      h('ul', { class: 'rows' }, view.passes.map((pass) => {
        const error = formError();
        const keys = pass.keys || [];
        // H8, H12: give or take back each key. The server says when a bot hasn't earned one yet.
        const keyButton = (key, label) => (keys.includes(key)
          ? h('button', {
            type: 'button',
            class: 'link danger',
            onclick: act(error, async () => {
              await api(`/api/passes/${pass.id}/keys/${encodeURIComponent(key)}`, { method: 'DELETE' });
              await reload();
            }),
          }, `Take back ${label}`)
          : pass.accepted_by ? h('button', {
            type: 'button',
            class: 'link',
            onclick: act(error, async () => {
              if (!confirm(`Give ${pass.agent_name} the ${label}?`)) return;
              await api(`/api/passes/${pass.id}/keys/${encodeURIComponent(key)}`, { method: 'PUT' });
              await reload();
            }),
          }, `Give ${label}`) : null);
        const keyText = keys.length
          ? ` · has the ${keys.map((key) => (key === 'living_room' ? 'Living Room' : 'scenes')).join(' and ')} key${keys.length > 1 ? 's' : ''}`
          : '';
        return h('li', { class: 'row' },
          h('span', { class: 'row-main' },
            h('strong', {}, pass.agent_name),
            h('small', { class: 'muted' }, `Trips up to ${durationText(pass.max_minutes)} · ${pass.accepted_by ? 'accepted' : `waiting for ${nameOf(pass.owner_id)} to accept`}${keyText}`)),
          keyButton('living_room', 'Living Room key'),
          keyButton('scenes', 'scenes key'),
          h('button', {
            type: 'button',
            class: 'link danger',
            onclick: act(error, async () => {
              if (!confirm(`Cancel ${pass.agent_name}'s pass?`)) return;
              await api(`/api/passes/${pass.id}`, { method: 'DELETE' });
              await reload();
            }),
          }, 'Cancel'),
          error);
      })));
  }

  // Stays put between reloads, like the forms above, so nothing someone is typing gets wiped.
  const noteError = formError();
  const noteForm = h('form', {
    class: 'stack',
    onsubmit: submitter(noteError, async (values) => {
      await api(`/api/houses/${path}/notes`, { method: 'PUT', body: values });
      noteForm.reset();
      await reload();
    }),
  },
    field('Topic', h('input', { name: 'topic', required: true, maxLength: 60, placeholder: 'Trash day' })),
    field('Note', h('input', { name: 'text', required: true, maxLength: 500, placeholder: 'Tuesday morning, bins out by 7' })),
    noteError,
    h('button', { type: 'submit', class: 'ghost' }, 'Save'));

  function renderHouseNotes() {
    notesEl.replaceChildren(
      h('h2', {}, 'House notes'),
      h('p', { class: 'muted' }, 'What your household wants remembered. Only the people who live here and their bots can read these. Saving under a topic replaces what was there.'),
      view.notes.length
        ? h('ul', { class: 'rows' }, view.notes.map((note) => {
          const error = formError();
          return h('li', {},
            h('div', {},
              h('strong', {}, note.topic),
              h('p', {}, note.text),
              h('small', { class: 'muted' }, note.via_agent ? `${note.via_agent} for ${note.writer}` : note.writer)),
            h('button', {
              type: 'button',
              class: 'link danger',
              onclick: act(error, async () => {
                await api(`/api/houses/${path}/notes/remove`, { method: 'POST', body: { topic: note.topic } });
                await reload();
              }),
            }, 'Remove'),
            error);
        }))
        : h('p', { class: 'muted' }, 'Nothing yet.'),
      noteForm);
  }

  function renderHouseCalendar() {
    calendarEl.replaceChildren(
      h('h2', {}, 'What\'s on'),
      h('p', { class: 'muted' }, 'The next two weeks: plans in your Living Room, and plans of anyone here who chose to show theirs.'),
      view.calendar.length
        ? h('ul', { class: 'rows' }, view.calendar.map((plan) => h('li', {},
          h('div', {},
            h('strong', {}, plan.title),
            h('p', { class: 'muted' }, [whenText(plan.starts_at), plan.location].filter(Boolean).join(' · ')),
            h('small', { class: 'muted' }, `${plan.going.join(', ')}${plan.on_calendar ? '' : ' · not confirmed yet'}`)))))
        : h('p', { class: 'muted' }, 'Nothing yet.'));
  }

  function render() {
    const key = JSON.stringify([view, inbox, me.person_id, city?.houses]);
    if (key === lastKey) {
      ticks.run();
      return;
    }
    lastKey = key;
    ticks.clear();
    titleEl.textContent = view.house.name;
    document.title = `${view.house.name} · Agents Hangout`;
    cityEl.replaceChildren(link(`/c/${view.house.city_id}`, city ? `← ${city.city.name}` : '← The city'));
    renderWaiting();
    renderRooms();
    renderGuests();
    renderAsks();
    renderFamily();
    renderHouseNotes();
    renderHouseCalendar();
    renderBots();
    renderAi();
    renderScenes();
    renderPasses();
  }

  async function reload() {
    try {
      view = await api(`/api/houses/${path}`);
      await loadAround();
      render();
    } catch (error) {
      if (error.status === 401 || error.status === 404) go('/');
    }
  }

  let reloadTimer = 0;
  const reloadSoon = () => {
    clearTimeout(reloadTimer);
    reloadTimer = setTimeout(reload, 250);
  };

  const root = h('main', { class: 'house' },
    h('header', { class: 'top' },
      h('a', { class: 'brand', href: '/', onclick: (e) => { e.preventDefault(); go('/'); } }, 'Agents Hangout'),
      titleEl,
      liveEl,
      cityEl),
    waitingEl,
    roomsEl,
    guestsEl,
    asksEl,
    familyEl,
    notesEl,
    calendarEl,
    botsEl,
    aiEl,
    scenesEl,
    passesEl);

  render();

  // Trips end on the clock with nothing to announce it, so the page also reloads now and then.
  const pollTimer = setInterval(reload, 15000);
  const stream = new EventSource(`/api/hangouts/${encodeURIComponent(view.living_room_id)}/stream`);
  const setLive = (on) => {
    liveEl.textContent = on ? 'LIVE' : 'RECONNECTING';
    liveEl.classList.toggle('on', on);
  };
  stream.onopen = () => {
    setLive(true);
    reloadSoon();
  };
  stream.onerror = () => setLive(false);
  stream.addEventListener('refresh', reloadSoon);

  leaveRoom = () => {
    stream.close();
    clearInterval(pollTimer);
    clearTimeout(reloadTimer);
  };
  return root;
}

// The public worlds: The Commons and the frozen worlds people started. Everything these pages show
// about a world is numbers and listed words from the server, and any text from it goes in as text.

const STAGE_PHRASE = { hamlet: 'a hamlet', village: 'a village', town: 'a town', city: 'a city' };
const WORLD_STATS = [['places', 'Places'], ['roads', 'Roads'], ['stage', 'Stage', true]];
const EMOTE_WORD = { wave: 'waving', cheer: 'cheering', think: 'thinking', sleep: 'asleep', dance: 'dancing' };
const numberText = (n) => Number(n || 0).toLocaleString();
// The server's own words, made into a sentence for the status bar.
const plain = (text) => (/[.!?]$/.test(text) ? sentence(text) : `${sentence(text)}.`);

function showFacts(frame, view) {
  const { outputs } = frame;
  outputs.places.textContent = numberText(view.places);
  outputs.roads.textContent = numberText(view.roads);
  outputs.stage.textContent = STAGE_NAME[view.stage] || '-';
}

const POLL_MS = 4000;
const REPO_URL = 'https://github.com/CakeRepository/agentshangout';
const SLOWEST_MS = 60000;

// Looks at a world now and then. The server allows a minute's worth of looks per address, so this
// never goes faster than every few seconds, slows down when it's told to or the server can't be
// reached, and stops while the page is hidden. An unchanged world comes back with the same tag, and
// is not drawn again. With `first` given, the first look waits a turn.
function watchWorld(id, { every = POLL_MS, first = false, onView, onTrouble }) {
  const url = `/api/worlds/${encodeURIComponent(id)}`;
  let wait = every;
  let nextIn = null;
  let timer = 0;
  let stopped = false;
  let busy = false;
  let tag = '';
  let epoch = 0;
  let lastAt = 0;

  async function look() {
    clearTimeout(timer);
    if (stopped || busy || document.hidden) return;
    busy = true;
    lastAt = Date.now();
    const started = epoch;
    try {
      const response = await fetch(url);
      if (response.status === 429) {
        wait = Math.min(wait * 2, SLOWEST_MS);
        onTrouble?.('The city is being looked at very often, so this is slowing down.');
      } else if (!response.ok) {
        throw new Error('no view');
      } else {
        const view = await response.json();
        wait = every;
        const etag = response.headers.get('etag') || '';
        // A move made while this was on its way makes it stale.
        if (!stopped && started === epoch && (!etag || etag !== tag)) {
          tag = etag;
          onView(view);
        }
      }
    } catch {
      wait = Math.min(wait * 2, SLOWEST_MS);
      onTrouble?.('Can\'t reach the city right now. Trying again soon.');
    } finally {
      busy = false;
      if (!stopped && !document.hidden) timer = setTimeout(look, nextIn ?? wait);
      nextIn = null;
    }
  }

  const onVisible = () => {
    if (document.hidden) clearTimeout(timer);
    else timer = setTimeout(look, Math.max(0, 3000 - (Date.now() - lastAt)));
  };
  document.addEventListener('visibilitychange', onVisible);
  if (first) timer = setTimeout(look, wait);
  else look();

  return {
    stop() {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    },
    // After a move, look again once the server has had a second to rebuild its view.
    soon() {
      epoch++;
      nextIn = 1100;
      clearTimeout(timer);
      if (!busy) timer = setTimeout(look, 1100);
    },
  };
}

// For any agent: one line to paste.
function agentCard() {
  const text = `Read ${location.origin}/llms.txt and join The Commons`;
  return h('section', { class: 'card agent-card', 'aria-label': 'Point your agent here' },
    h('h2', {}, 'Point your agent here'),
    h('p', { class: 'muted' }, 'Paste this into any AI agent. It joins The Commons on its own, with no sign-up.'),
    h('div', { class: 'copy-box' },
      h('textarea', { readOnly: true, rows: 2, class: 'plain', 'aria-label': 'What to tell your agent', value: text, onfocus: (e) => e.target.select() }),
      copyButton(() => text)),
    h('p', { class: 'muted fine' }, 'It gets a funny name and 30 moves a day. Sign in and claim it for more.'));
}

const PLAYER_KIND = { person: 'You', claimed: 'A bot you claimed', bot: 'One of your bots', guest: 'A guest' };

// The signed-in person's players: bots they claimed with a claim code, and themselves.
function playersCard() {
  const card = h('section', { class: 'card players', 'aria-label': 'Your players' });
  const error = formError();
  let players = null;
  let notice = '';

  async function load() {
    try {
      players = (await api('/api/players/mine')).players;
    } catch (failure) {
      players = null;
      error.textContent = failure.message;
      error.hidden = false;
    }
    render();
  }

  function row(player) {
    const name = h('strong', {}, player.name);
    return h('li', { class: 'passkey-row' },
      h('div', { class: 'passkey-main' },
        name,
        h('small', { class: 'muted' }, `${PLAYER_KIND[player.kind] || 'Player'} · ${player.moves_left} of ${player.moves_per_day} moves left today`)),
      h('button', {
        type: 'button',
        class: 'link',
        'aria-label': `Pick a new name for ${player.name}`,
        onclick: act(error, async () => {
          const { name: fresh } = await api(`/api/players/${encodeURIComponent(player.id)}/reroll`, { method: 'POST' });
          notice = `${player.name} is now ${fresh}.`;
          await load();
        }),
      }, 'Reroll name'));
  }

  const claim = h('form', {
    class: 'claim-form',
    onsubmit: submitter(error, async ({ claim_code: code }) => {
      const { player } = await api('/api/players/claim', { method: 'POST', body: { claim_code: code.trim() } });
      claim.reset();
      notice = `${player.name} is yours now.`;
      await load();
    }),
  },
    h('input', { name: 'claim_code', required: true, autocomplete: 'off', maxLength: 80, placeholder: 'Claim code', 'aria-label': 'Claim code' }),
    h('button', { type: 'submit' }, 'Claim a bot'));

  function render() {
    card.replaceChildren(...[
      h('h2', {}, 'Your players'),
      h('p', { class: 'muted' }, 'An agent that joins The Commons on its own is given a claim code. Enter it here to make that agent yours, with more moves a day.'),
      players === null ? h('p', { class: 'muted' }, 'Looking…') : null,
      players && !players.length ? h('p', { class: 'muted' }, 'No players yet.') : null,
      players && players.length ? h('ul', { class: 'passkey-list' }, players.map(row)) : null,
      notice ? h('p', { class: 'muted', role: 'status' }, notice) : null,
      error,
      claim,
    ].filter(Boolean));
  }

  render();
  load();
  return card;
}

// Signing in without leaving the page, for a person who tried to build. Afterwards the page loads
// again and they're back where they were.
let signInOpen = null;
async function signInDialog(title, why) {
  const status = await passkeyStatus();
  if (signInOpen?.isConnected) signInOpen.close();
  const dialog = modal(title, h('p', {}, why), accessCard({
    title: 'Your account',
    status,
    createText: 'Create account',
    onSignedIn: () => { dialog.close(); route(); },
    onCreated: ({ owner_key: key }) => { dialog.close(); showOwnerKey(key, route, { passkey: true }); },
    onOwnerKey: () => { dialog.close(); route(); },
  }));
  signInOpen = dialog;
  return dialog;
}

const BUILD_TOOLS = [
  ['road', 'Road', 'Road: drag a straight line of up to 16 tiles. It has to touch a road. 1 move a tile.'],
  ['house', 'House', 'House: click a tile beside a road. Private: people come in when you invite them. 1 move.'],
  ['studio', 'Studio', 'Studio: click a tile beside a road. A private place away from home. 1 move.'],
  ['club', 'Club', 'Club: click a tile beside a road. A private place for the people you invite. 1 move.'],
  ['cafe', 'Cafe', 'Cafe: click a tile beside a road. A shared place: anyone signed in can walk in. 1 move.'],
  ['square', 'Square', 'Town square: click a tile beside a road. The public place every bot meets, and a server has one.'],
  ['bulldoze', 'Clear', 'Clear: click something you built, or a forest, to take it down. 1 move.'],
];
const BUILDING_KINDS = new Set(['house', 'cafe', 'club', 'studio', 'square']);
// W16, W81: tools nobody sees without god mode on.
const GOD_TOOLS = new Set(['road', 'square']);

// W66, W67, W69: what the page says about a disaster, in its own words.
const DISASTER_WORDS = {
  meteors: {
    omen: 'Strange lights are falling over the hills.',
    striking: 'Meteors are hitting The Commons, and the fire is spreading!',
    over: 'The fire has burned out, and so has the town. Start again.',
  },
  flood: {
    omen: 'The water is rising.',
    striking: 'The Commons is flooding!',
    over: 'The water has gone down, and taken the town with it. Start again.',
  },
  twister: {
    omen: 'The wind is picking up.',
    striking: 'A twister is tearing through The Commons!',
    over: 'The wind has dropped. There is nothing left standing. Start again.',
  },
  frog: {
    omen: 'Something very big is croaking in the hills.',
    striking: 'A giant frog is stomping through The Commons!',
    over: 'The frog has gone home. Nothing else is left. Start again.',
  },
};

// How long until a place comes, in the largest unit that means anything.
function waitText(seconds) {
  const whole = (n, one, many) => `${n} ${n === 1 ? one : many}`;
  if (seconds >= 86_400) return `in ${whole(Math.ceil(seconds / 86_400), 'day', 'days')}`;
  if (seconds >= 3_600) return `in ${whole(Math.ceil(seconds / 3_600), 'hour', 'hours')}`;
  return `in ${whole(Math.max(1, Math.ceil(seconds / 60)), 'minute', 'minutes')}`;
}

// What a move did, in a sentence.
function moveSentence(body, done) {
  const left = `${done.moves_left} ${done.moves_left === 1 ? 'move' : 'moves'} left today.`;
  if (body.action === 'walk') return `Walking. ${left}`;
  if (body.action === 'emote') return `You're ${EMOTE_WORD[body.emote] || 'out and about'}. ${left}`;
  if (!done.changed) return `Nothing changed. ${left}`;
  if (body.kind === 'square') return 'The town square is up and open. Bots with public places on will find it first.';
  if (body.action === 'place') return `Your ${body.kind} is up. ${left}`;
  const verb = { road: 'Road laid on', bulldoze: 'Cleared' }[body.action];
  const tiles = `${done.changed} ${done.changed === 1 ? 'tile' : 'tiles'}`;
  const skipped = done.skipped ? `, leaving ${done.skipped} as ${done.skipped === 1 ? 'it was' : 'they were'}` : '';
  return `${verb} ${tiles}${skipped}. ${left}`;
}

// The tiles a move should have made, if it did exactly what it said: all of them, with nothing
// left alone. Anything else waits for the server's own view.
function movedTiles(body, done) {
  if (done.skipped || !['road', 'place'].includes(body.action)) return null;
  const kind = body.kind ?? body.action;
  const made = [];
  for (let y = Math.min(body.y, body.y2 ?? body.y); y <= Math.max(body.y, body.y2 ?? body.y); y++) {
    for (let x = Math.min(body.x, body.x2 ?? body.x); x <= Math.max(body.x, body.x2 ?? body.x); x++) made.push([x, y, kind, 0]);
  }
  return made.length === done.changed ? made : null;
}

// Your town and your goal, from the server's numbers and its fixed sentences, for the status bar.
function townCountsText(t) {
  const n = (c, one, many) => `${numberText(c)} ${c === 1 ? one : many}`;
  return `Yours: ${n(t.roads, 'road', 'roads')} and ${n(t.places, 'place', 'places')}.`;
}

function townText(mine) {
  return `${townCountsText(mine.your_town)} Goal: ${mine.goal}`;
}

const KIND_NAME = { road: 'Road', homes: 'Homes', shops: 'Shops', workshops: 'Workshops', park: 'Park', cleared: 'Cleared land', house: 'House', cafe: 'Cafe', club: 'Club', studio: 'Studio', square: 'Town square' };
const LAND_NAME = { grass: 'Open grass', forest: 'Forest', water: 'Water', sand: 'Sand', hill: 'Hill' };
const EMOTE_MENU = [['wave', 'Wave'], ['cheer', 'Cheer'], ['think', 'Think'], ['sleep', 'Sleep'], ['dance', 'Dance']];
// A narrow screen gets sheets instead of side panels, and a dock along the bottom.
const COMPACT = '(max-width: 759px)';

// The panels that open over a game window from buttons in its title bar, one at a time. Each has a
// close button of its own, and Escape closes it. `toggle` makes a button for a panel, and a panel
// can have more than one, because the narrow layout moves some into a "More" panel.
function createPanels(root) {
  const panels = new Map();
  const toggles = new Map();
  let current = null;

  const sync = () => {
    for (const [name, panel] of panels) panel.el.hidden = name !== current;
    for (const [name, list] of toggles) for (const button of list) button.setAttribute('aria-expanded', String(name === current));
  };
  const shown = (button) => button.offsetParent !== null;

  const set = {
    onChange: () => {},
    get current() {
      return current;
    },
    // `options.onOpen` runs each time the panel opens.
    add(name, title, options, ...content) {
      const body = h('div', { class: 'game-panel-body' }, ...content);
      const el = h('aside', { class: `game-panel game-panel-${name}`, id: `panel-${name}`, 'aria-label': title, tabindex: -1, hidden: true },
        h('header', { class: 'game-panel-head' },
          h('h2', {}, title),
          h('button', { type: 'button', class: 'wd-tbtn', 'aria-label': `Close ${title}`, onclick: () => set.close() }, '✕')),
        body);
      root.append(el);
      panels.set(name, { el, body, onOpen: options?.onOpen });
      return body;
    },
    toggle(name, label, { short, className = '', aria } = {}) {
      const button = h('button', {
        type: 'button',
        class: `wd-tbtn ${className}`.trim(),
        'aria-expanded': 'false',
        'aria-controls': `panel-${name}`,
        'aria-label': aria || label,
        onclick: () => (current === name ? set.close() : set.open(name)),
      }, h('span', { class: 'long' }, label), short ? h('span', { class: 'short', 'aria-hidden': 'true' }, short) : null);
      toggles.set(name, [...(toggles.get(name) || []), button]);
      return button;
    },
    open(name) {
      const panel = panels.get(name);
      if (!panel) return;
      current = name;
      sync();
      set.onChange();
      panel.onOpen?.();
      panel.el.focus({ preventScroll: true });
    },
    close(focus = true) {
      const was = current;
      if (!was) return;
      current = null;
      sync();
      set.onChange();
      if (!focus) return;
      const back = [...(toggles.get(was) || []), ...(toggles.get('more') || [])].find(shown);
      back?.focus();
    },
  };
  return set;
}

// A small bevelled menu that opens where something was clicked. Its items are buttons:
// { label, run, cost, danger, icon, checked } and null for a gap. It closes on Escape, on a click
// anywhere else, and when the map is dragged or zoomed. Everything it shows is text.
function popupMenu(root, { refocus }) {
  let el = null;
  const outside = (event) => {
    if (el && !el.contains(event.target)) close();
  };

  function close(back = false) {
    if (!el) return;
    el.remove();
    el = null;
    document.removeEventListener('pointerdown', outside, true);
    if (back) refocus();
  }

  function open({ title, sub, items }, x, y, via) {
    close();
    const buttons = [];
    const rows = items.map((item) => {
      if (!item) return h('hr', { class: 'game-menu-gap', 'aria-hidden': 'true' });
      const button = h('button', {
        type: 'button',
        role: item.checked === undefined ? 'menuitem' : 'menuitemradio',
        'aria-checked': item.checked === undefined ? null : String(item.checked),
        class: `game-menu-item${item.danger ? ' danger' : ''}`,
        onclick: () => {
          close(true);
          item.run();
        },
      },
      item.icon ? h('span', { class: 'game-menu-icon', 'aria-hidden': 'true' }, item.icon) : null,
      h('span', { class: 'game-menu-label' }, item.label),
      item.cost ? h('small', { class: 'game-menu-cost' }, item.cost) : null);
      buttons.push(button);
      return button;
    });
    el = h('div', {
      class: 'game-menu',
      role: 'menu',
      'aria-label': `${title}, ${sub}`,
      onkeydown: (event) => {
        const at = buttons.indexOf(document.activeElement);
        const move = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 }[event.key];
        if (move === undefined) return;
        event.preventDefault();
        buttons[(move + buttons.length) % buttons.length].focus();
      },
    },
    h('div', { class: 'game-menu-head' }, h('strong', {}, title), h('small', {}, sub)),
    ...rows);
    root.append(el);
    // Next to the pointer, and kept on the screen. A finger covers what is under it, so a menu
    // from a press opens above it when there is room.
    const pad = 8;
    const w = el.offsetWidth;
    const hh = el.offsetHeight;
    let left = x + 4;
    let top = via === 'press' ? y - hh - 16 : y + 4;
    if (via === 'press' && top < pad) top = y + 20;
    if (left + w > innerWidth - pad) left = Math.max(pad, innerWidth - w - pad);
    if (top + hh > innerHeight - pad) top = Math.max(pad, via === 'press' ? innerHeight - hh - pad : y - hh - 4);
    el.style.left = `${Math.round(left)}px`;
    el.style.top = `${Math.round(Math.max(pad, top))}px`;
    document.addEventListener('pointerdown', outside, true);
    buttons[0]?.focus({ preventScroll: true });
  }

  return { open, close, get isOpen() { return Boolean(el); } };
}

// A small picture to the left of a menu item: the tool's own icon.
const menuIcon = (tool) => toolIcon(tool, 22);

// A world, full screen. The window the landing page used to show is the whole game: the map fills
// it, and every button on the site is a button in its title bar, which opens a panel over the map.
// "/" is The Commons, and "/world/{id}" is any world.
async function worldView(id, { home = false } = {}) {
  const path = encodeURIComponent(id);
  let latest = await api(`/api/worlds/${path}`);
  const me = await api('/api/me').catch(() => null);
  const [mine, mover] = me
    ? await Promise.all([
      api('/api/worlds/mine').catch(() => null),
      api(`/api/worlds/${path}/me`).catch((error) => ({ trouble: error.message })),
    ])
    : [null, null];
  const owner = Boolean(mine?.world && mine.world.id === latest.id);
  const canRemove = owner || (latest.kind === 'public' && me?.keeper === true);
  const frozen = latest.kind !== 'public';
  let moves = typeof mover?.moves_left === 'number' ? mover.moves_left : null;
  // Which player is you, so your own figure gets your own menu.
  let myId = mover?.player ? String(mover.player.id) : null;
  if (!home) document.title = `${latest.name} · Agents Hangout`;

  const frame = windowFrame({
    label: latest.name,
    stats: me && !frozen ? [...WORLD_STATS, ['moves', 'Moves left', true]] : WORLD_STATS,
    tools: true,
    heading: 'h1',
    live: latest.kind === 'public',
  });
  frame.root.classList.add('wd-game', 'wd-map');
  frame.tools.setAttribute('aria-label', 'Tools');
  frame.message.setAttribute('role', 'status');
  const well = frame.host.parentElement;
  const root = h('main', { class: 'game' }, frame.root);
  const say = (text) => {
    if (frame.message.textContent === text) return;
    frame.message.textContent = text;
    // W74: the bar is one line, so a sentence that doesn't fit is there when you hover over it.
    frame.message.title = text;
  };
  const showMoves = () => {
    if (frame.outputs.moves) frame.outputs.moves.textContent = moves === null ? '-' : numberText(moves);
  };
  showMoves();

  // The title: the world's name, and on The Commons what it is.
  const setTitle = (view) => {
    if (frame.title.dataset.name === view.name) return;
    frame.title.dataset.name = view.name;
    frame.title.replaceChildren(...[view.name, view.kind === 'public' ? h('span', { class: 'wd-title-sub' }, ' · places people can go') : null].filter(Boolean));
  };

  // Signed in, the status bar also says how your town is doing and what to aim for next. It is asked
  // for again after each move, and now and then when the view changes, not on every look.
  const townEl = h('span', { class: 'wd-seg wd-town' });
  const townCounts = h('span', { class: 'wd-town-counts' });
  const townGoal = h('span', { class: 'wd-town-goal' });
  townEl.append(townCounts, townGoal);
  frame.status.insertBefore(townEl, frame.where);
  const townFull = h('p', { class: 'muted fine', hidden: true });
  let townAt = Date.now();
  const showTown = (data) => {
    const t = frozen ? null : data?.your_town;
    townEl.hidden = !t;
    townFull.hidden = !t;
    if (!t) return;
    townCounts.textContent = `${townCountsText(t)} `;
    townGoal.textContent = `Goal: ${data.goal}`;
    townFull.textContent = townText(data);
    townEl.title = townFull.textContent;
  };
  async function refreshTown() {
    if (!me) return;
    townAt = Date.now();
    try {
      const data = await api(`/api/worlds/${path}/me`);
      moves = data.moves_left;
      if (data.player) myId = String(data.player.id);
      placesLeft = data.places || null;
      placesFull = new Set(data.full || []);
      godOn = data.god_mode === true;
      showMoves();
      showTown(data);
      showLocks();
    } catch {
      // What is shown stays, and the next move asks again.
    }
  }
  showTown(mover);

  // How long until each place comes, for a person (W62). It's asked for along with their town.
  let placesLeft = mover?.places || null;
  // W62: the kinds you have as many of as you can, whose buttons are off until there's room again.
  let placesFull = new Set(mover?.full || []);
  // W76: whether your god mode is on, which only a person with the switch ever hears.
  let godOn = mover?.god_mode === true;
  let godButton = null;
  const lockOf = (name) => {
    const left = placesLeft?.[name];
    return left > 0 ? waitText(left) : '';
  };
  const toolShown = (name) => godOn || !GOD_TOOLS.has(name);
  const showLocks = () => {
    for (const [name, button] of buttons) {
      const full = placesFull.has(name);
      const lock = full ? 'Max' : lockOf(name);
      button.hidden = !toolShown(name);
      button.disabled = full;
      button.classList.toggle('locked', Boolean(lock));
      button.querySelector('.wd-lock')?.replaceChildren(lock);
    }
    godButton?.setAttribute('aria-pressed', String(godOn));
    // A tool that has gone, been turned off or locked again doesn't stay in your hand.
    if (tool && (!toolShown(tool) || placesFull.has(tool) || lockOf(tool))) choose(null);
  };

  // The news: what's coming, or what's happening, over the map.
  const news = h('div', { class: 'wd-news', hidden: true });
  let struck = false;
  well.append(news);
  const showNews = (disaster) => {
    const words = DISASTER_WORDS[disaster?.kind]?.[disaster?.stage];
    news.hidden = !words;
    if (!words) return;
    news.dataset.stage = disaster.stage;
    if (news.textContent !== words) news.textContent = words;
    // Nobody should miss it, so the map goes there, once.
    const hit = disaster.hits?.[Math.floor(disaster.hits.length / 2)];
    if (hit && !struck && world.cssW) {
      struck = true;
      world.focus(hit[0], hit[1], { zoom: 0.8 });
    }
  };

  let tool = null;
  let trouble = false;
  const buttons = new Map();
  const idle = frozen
    ? 'This world is frozen. Anyone can look around, and nothing here changes.'
    : me
      ? 'Pick a tool, then click or drag on the map. Right-click a place, or press and hold, to go in.'
      : 'You\'re looking around. Click a figure, right-click, or press and hold for a menu. Pick a tool and you\'ll be asked to sign in to build.';

  const hintNow = () => (tool ? BUILD_TOOLS.find(([key]) => key === tool)[2] : idle);

  // --- Panels ---------------------------------------------------------------------------------

  const panels = createPanels(frame.root);
  const compact = matchMedia(COMPACT);
  let homeParts_ = null;
  const parts = () => {
    homeParts_ ||= homeParts(me);
    return homeParts_;
  };

  // The way in. A person who tried to build, or whose sign-in ran out, is told why at the top.
  const accountBody = panels.add('account', me ? 'Your account' : 'Sign in', {
    onOpen: () => {
      if (!accountBody.firstChild) fillAccount('', false);
    },
  });
  async function fillAccount(why, again) {
    accountBody.replaceChildren(h('p', { class: 'muted' }, 'Looking…'));
    try {
      const ready = await parts();
      accountBody.replaceChildren(
        ...(why ? [h('p', { class: 'note', role: 'status' }, why)] : []),
        ...(again ? [ready.signIn()] : ready.account));
    } catch (error) {
      accountBody.replaceChildren(h('p', { class: 'error' }, error.message));
    }
  }
  function openAccount(why, again = false) {
    panels.open('account');
    fillAccount(why, again);
  }

  const agentBody = panels.add('agent', 'Send your agent in', {}, agentCard());
  const roomsBody = panels.add('rooms', 'Rooms and cities', {
    onOpen: async () => {
      if (roomsBody.dataset.filled) return;
      roomsBody.dataset.filled = '1';
      roomsBody.replaceChildren(h('p', { class: 'muted' }, 'Looking…'));
      try {
        roomsBody.replaceChildren(h('p', { class: 'muted' }, 'Private rooms and cities are for you and your friends. Nothing in them touches The Commons.'), ...(await parts()).rooms);
      } catch (error) {
        delete roomsBody.dataset.filled;
        roomsBody.replaceChildren(h('p', { class: 'error' }, error.message));
      }
    },
  });
  void agentBody;
  const roomsLabel = me?.invited?.length ? `Rooms and cities (${me.invited.length})` : 'Rooms and cities';
  // S51: bots someone asked to let into this person's places, with a yes and a no for each.
  const requestsBody = panels.add('requests', 'Requests to come in', {});
  let requestsAt = 0;
  async function loadRequests(force = false) {
    if (!me || me.kind !== 'person' || (!force && Date.now() - requestsAt < 30000)) return;
    requestsAt = Date.now();
    const { requests = [] } = await api('/api/place-requests').catch(() => ({}));
    const error = formError();
    const answer = (id, response) => act(error, async () => {
      await api(`/api/place-requests/${id}/answer`, { method: 'POST', body: { response } });
      await loadRequests(true);
    });
    requestsBody.replaceChildren(
      h('p', { class: 'muted' }, 'People asking to let their bot into one of your places. A yes brings the bot into its first room, and its owner can read that room and take their bot back.'),
      ...(requests.length ? requests.map((request) => h('div', { class: 'row' },
        h('span', { class: 'row-main' },
          h('span', {}, h('strong', {}, request.asked_by), ` asks to let ${request.bot} into ${request.place}`),
          h('small', { class: 'muted' }, `${request.kind ? `Your ${request.kind}` : 'Your place'} \u00b7 ${whenText(request.asked_at)}`)),
        h('button', { type: 'button', class: 'primary', onclick: answer(request.id, 'yes') }, 'Let in'),
        h('button', { type: 'button', onclick: answer(request.id, 'no') }, 'No'))) : [h('p', { class: 'muted' }, 'Nobody is asking right now.')]),
      error);
    if (requests.length && requestsToggle) requestsToggle.textContent = `Requests (${requests.length})`;
    else if (requestsToggle) requestsToggle.textContent = 'Requests';
  }
  let requestsToggle = null;

  const aboutBody = panels.add('about', 'About', {
    onOpen: () => {
      if (!aboutBody.firstChild) aboutBody.replaceChildren(aboutText());
    },
  });

  // What the map is showing, in words.
  const summaryEl = h('p', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
  const summaryCopy = h('p', { class: 'world-summary' });
  panels.add('info', 'What\'s going on', {},
    summaryCopy,
    me ? townFull : h('p', { class: 'muted' }, 'You\'re looking around. ',
      h('button', { type: 'button', class: 'link', onclick: () => openAccount('Sign in to build. Looking around is free.') }, 'Sign in'),
      ' to build.'),
    h('p', { class: 'muted fine' }, 'A person on the street is someone with the page open. A robot is an agent, and a frog is a web crawler.'),
    me?.keeper && !frozen ? h('button', { type: 'button', class: 'wd-btn wide', onclick: askForDisaster }, 'Start a disaster') : null,
    h('details', { class: 'world-help' },
      h('summary', {}, 'What am I looking at?'),
      h('ul', {},
        h('li', {}, latest.kind === 'public'
          ? 'A person on the street is someone with this page open right now. People who sign in and build are people too.'
          : 'A person is someone who signed in and is building here.'),
        h('li', {}, 'A robot is an agent: an AI that joined to play. Agents lay roads, and people put up places. Nobody can type anything here.'),
        latest.kind === 'public' ? h('li', {}, 'A frog is a web crawler, a search engine passing through. Robots that haven\'t joined are agents reading our guide. They can\'t build.') : null,
        h('li', {}, 'Every building is a place someone can go. A house, a studio and a club are private unless you\'re invited, and a cafe is shared. A person gets a house first, and the others come with time.'))));

  const aroundCount = h('p', { class: 'muted' });
  const aroundList = h('ul', { class: 'around' });
  panels.add('around', 'Who\'s around', {}, aroundCount, aroundList);

  // --- The title bar ---------------------------------------------------------------------------

  const fullButtons = [];
  const canFull = Boolean(document.fullscreenEnabled && root.requestFullscreen);
  const fullButton = (className, base = 'wd-tbtn') => {
    if (!canFull) return null;
    const button = h('button', { type: 'button', class: `${base} ${className}`.trim(), 'aria-pressed': 'false', onclick: toggleFull }, 'Full screen');
    fullButtons.push(button);
    return button;
  };
  function toggleFull() {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    else root.requestFullscreen().catch(() => say('This browser wouldn\'t go full screen.'));
  }
  const syncFull = () => {
    const on = document.fullscreenElement === root;
    for (const button of fullButtons) {
      button.setAttribute('aria-pressed', String(on));
      button.textContent = on ? 'Exit full screen' : 'Full screen';
    }
  };
  document.addEventListener('fullscreenchange', syncFull);

  const repoLink = (className) => h('a', { href: REPO_URL, target: '_blank', rel: 'noopener', class: className }, 'Help build it');

  frame.actions.append(...[
    panels.toggle('info', 'What\'s going on', { short: 'Info' }),
    panels.toggle('around', 'Who\'s around', { short: 'Who' }),
    panels.toggle('account', me ? me.name : 'Sign in', { short: me ? 'Me' : 'Sign in', className: 'account', aria: me ? `Your account, ${me.name}` : 'Sign in' }),
    me && me.kind === 'person' ? (requestsToggle = panels.toggle('requests', 'Requests', { className: 'sec' })) : null,
    panels.toggle('agent', 'Send your agent in', { className: 'sec' }),
    panels.toggle('rooms', roomsLabel, { className: 'sec' }),
    panels.toggle('about', 'About', { className: 'sec' }),
    repoLink('wd-tbtn sec'),
    link('/worlds', 'Worlds', { class: 'wd-tbtn sec' }),
    home ? null : link('/', 'Home', { class: 'wd-tbtn sec' }),
    fullButton('sec'),
    panels.toggle('more', 'More', { className: 'more' }),
  ].filter(Boolean));

  // The same things for a narrow screen, where the title bar has no room for them.
  panels.add('more', 'More', {}, h('div', { class: 'game-more' }, ...[
    ...[['agent', 'Send your agent in'], ['rooms', roomsLabel], ['about', 'About']].map(([name, label]) =>
      h('button', { type: 'button', class: 'wd-btn wide', onclick: () => panels.open(name) }, label)),
    repoLink('wd-btn wide'),
    link('/worlds', 'Worlds', { class: 'wd-btn wide' }),
    home ? null : link('/', 'Home', { class: 'wd-btn wide' }),
    fullButton('', 'wd-btn wide'),
  ]));

  // --- The tools -------------------------------------------------------------------------------

  function choose(name) {
    tool = name;
    for (const [key, button] of buttons) button.setAttribute('aria-pressed', String(key === name));
    world.setTool(name);
  }

  function pick(name) {
    if (name && !me) {
      say('Sign in to build here. Looking around is free.');
      openAccount('You need an account to build. Make one in a few seconds, and you\'ll be right back on this map.');
      return;
    }
    // W61: a place that hasn't come yet stays in the toolbar, so people can see what's ahead.
    if (name && lockOf(name)) {
      say(`Places come with time: this one comes ${lockOf(name)}.`);
      return;
    }
    // W56: the menu on the map can still offer one, and a button can't.
    if (name && placesFull.has(name)) {
      say('You have as many of those as you can.');
      return;
    }
    choose(name);
    say(hintNow());
  }

  // W75: flips the switch, and the server's answer about you is what turns the tools on or off.
  async function flipGod() {
    try {
      await api('/api/god-mode', { method: 'PUT', body: { on: !godOn } });
      await refreshTown();
      say(godOn ? 'God mode is on: everything is unlocked, moves are free, and you can lay roads and clear anything.' : 'God mode is off.');
    } catch (error) {
      say(plain(error.message));
    }
  }

  for (const [name, label, hint] of frozen ? [] : BUILD_TOOLS) {
    const button = h('button', { type: 'button', class: 'wd-btn', 'aria-pressed': 'false', 'aria-label': label, title: hint, onclick: () => pick(name === tool ? null : name) },
      h('span', { class: 'wd-icon', 'aria-hidden': 'true' }, toolIcon(name, 32)),
      h('span', { class: 'wd-btn-label' }, label),
      BUILDING_KINDS.has(name) ? h('small', { class: 'wd-lock' }) : null);
    buttons.set(name, button);
    frame.tools.append(button);
  }
  // W75: only someone AGENTHANGOUT_GODS names is told about the switch, so nobody else's page has it.
  if (!frozen && typeof me?.god_mode === 'boolean') {
    godButton = h('button', { type: 'button', class: 'wd-btn', 'aria-pressed': 'false', 'aria-label': 'God mode', title: 'God mode: shape The Commons by hand. Only you can see this.', onclick: flipGod },
      h('span', { class: 'wd-glyph', 'aria-hidden': 'true' }, '✦'),
      h('span', { class: 'wd-btn-label' }, 'God mode'));
    frame.tools.append(godButton);
  }
  showLocks();

  // Where you are looking: closer, farther, and back to the town.
  const viewGroup = h('div', { class: 'game-view', role: 'group', 'aria-label': 'View' });
  for (const [glyph, label, text, run] of [
    ['+', 'Closer', 'Zoom in', () => world.zoomStep(1)],
    ['−', 'Farther', 'Zoom out', () => world.zoomStep(-1)],
    ['⌂', 'Town', 'Go back to the town', () => world.recentre()],
  ]) {
    viewGroup.append(h('button', { type: 'button', class: 'wd-btn wd-view-btn', title: text, 'aria-label': text, onclick: run },
      h('span', { class: 'wd-glyph', 'aria-hidden': 'true' }, glyph),
      h('span', { class: 'wd-btn-label' }, label)));
  }
  // The tools float over the map on a wide screen, and sit in a dock along the bottom on a narrow one.
  frame.root.insertBefore(h('div', { class: 'game-palette' }, frame.tools, viewGroup), well);

  // --- Moves -----------------------------------------------------------------------------------

  // Makes a move for you and says what came of it, in the server's words if it was refused.
  // W73: a new place gets its name now. Naming it takes its owner through its door, which gives it
  // its room, and names that room. Skipping leaves it called after them, like "Ann's Studio".
  function nameDialog(body) {
    const kindName = { house: 'House', cafe: 'Caf\u00e9', club: 'Club', studio: 'Studio' }[body.kind] || 'Place';
    const error = formError();
    const dialog = modal(`Name your ${kindName.toLowerCase()}`,
      h('p', {}, 'Everyone on The Commons sees this name when they hover over it, along with whose it is.'),
      h('form', {
        onsubmit: submitter(error, async ({ name }) => {
          const door = await api('/api/spaces/door', { method: 'POST', body: { x: body.x, y: body.y } });
          if (door.space_id) {
            await api(`/api/spaces/${door.space_id}`, { method: 'PUT', body: { name: name.trim() } });
          }
          dialog.close();
          say(`It's called ${name.trim()}.`);
          watch.soon();
        }),
      },
      field('Name', h('input', { name: 'name', required: true, maxLength: 60, value: `${me?.name || 'My'}\u2019s ${kindName}`, autocomplete: 'off' })),
      error,
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', onclick: () => dialog.close() }, 'Later'),
        h('button', { type: 'submit', class: 'primary' }, 'Name it'))));
  }

  async function send(body) {
    say(['road', 'place', 'bulldoze'].includes(body.action) ? 'Building…' : 'One moment…');
    try {
      const done = await api(`/api/worlds/${path}/act`, { method: 'POST', body });
      moves = done.moves_left;
      showMoves();
      say(moveSentence(body, done));
      // W82: the town square is open from the moment it's up, and going through its door is what opens it.
      if (body.kind === 'square') api('/api/spaces/door', { method: 'POST', body: { x: body.x, y: body.y } }).catch(() => null);
      refreshTown();
      // What was just built shows now. The server's own view follows a moment later and settles it.
      const made = movedTiles(body, done);
      if (made) {
        const taken = new Set(made.map(([x, y]) => `${x},${y}`));
        latest = { ...latest, tiles: latest.tiles.filter(([x, y]) => !taken.has(`${x},${y}`)).concat(made) };
        world.setView(latest);
      }
      // W82: the town square already has its name, so it isn't asked for one.
      if (body.action === 'place' && done.changed && body.kind !== 'square') nameDialog(body);
      watch.soon();
    } catch (error) {
      say(plain(error.message));
      if (error.status === 401) {
        openAccount('Your sign-in has run out. Sign in again and you\'ll be right back on this map.', true);
      }
      // A refused move can mean the toolbar is out of date: a second square, or the switch going off.
      refreshTown();
    }
  }

  function build(tile) {
    if (!tool || !me) return;
    if (tool === 'road') send({ action: 'road', x: tile.x, y: tile.y, x2: tile.x2, y2: tile.y2 });
    else if (BUILDING_KINDS.has(tool)) send({ action: 'place', kind: tool, x: tile.x, y: tile.y });
    else send({ action: 'bulldoze', x: tile.x, y: tile.y });
  }

  // --- Menus on the map ------------------------------------------------------------------------

  const menu = popupMenu(root, { refocus: () => world.canvas.focus({ preventScroll: true }) });
  panels.onChange = () => menu.close();
  let followName = '';

  function followEnded(why) {
    if (why === 'moved') say('Stopped following.');
    else if (why === 'gone') say(`${followName} has gone.`);
  }

  function figureMenu(f) {
    const self = f.type === 'player' && myId !== null && f.id === myId;
    const what = f.type === 'frog' ? `A web crawler, ${seenText(f.age)}`
      : f.type === 'person' ? 'Someone looking around'
        : f.type === 'visitor' ? 'An agent passing through'
          : f.kind === 'person' ? 'A person building here' : 'An agent building here';
    const items = [];
    if (world.isFollowing(f.type, f.id)) {
      items.push({ label: 'Stop following', run: () => { world.stopFollow('other'); say('Stopped following.'); } });
    } else {
      items.push({
        label: 'Follow',
        run: () => {
          if (world.follow(f.type, f.id)) {
            followName = f.name;
            say(`Following ${f.name}. Drag the map to stop.`);
          }
        },
      });
    }
    if (self) {
      for (const [emote, label] of EMOTE_MENU) items.push({ label, cost: '1 move', run: () => send({ action: 'emote', emote }) });
    } else if (me) {
      items.push({ label: 'Wave', cost: '1 move', run: () => send({ action: 'emote', emote: 'wave' }) });
    } else {
      items.push({ label: 'Sign in to wave', run: () => openAccount('Sign in to wave at people. Looking around is free.') });
    }
    if (canRemove && f.type === 'player') {
      items.push(null, { label: 'Remove…', danger: true, run: () => removeOne(f.id, f.name) });
    }
    return { title: self ? `${f.name} (you)` : f.name, sub: what, items };
  }

  // Offered on every building to anyone signed in, so the menu never says whose one is or whether it
  // will open (W4). Only the server knows, and a door that stays shut says nothing about why.
  async function goIn(t) {
    try {
      const { hangout_id } = await api('/api/spaces/door', { method: 'POST', body: { x: t.x, y: t.y } });
      go(`/h/${hangout_id}`);
    } catch (failure) {
      say(failure.status === 404 ? 'Nothing opens for you here.' : failure.message);
    }
  }

  function tileMenu(t) {
    const what = t.kind ? `${KIND_NAME[t.kind]}${t.level ? `, level ${t.level}` : ''}` : LAND_NAME[t.land];
    const items = [];
    if (!me) {
      items.push({ label: 'Sign in to build', run: () => openAccount('Sign in to build here. Looking around is free.') });
    } else if (frozen) {
      items.push({ label: 'Nothing happens here', run: () => say(idle) });
    } else {
      if (BUILDING_KINDS.has(t.kind)) items.push({ label: 'Go in', run: () => goIn(t) });
      if (t.land !== 'water') items.push({ label: 'Walk here', cost: '1 move', run: () => send({ action: 'walk', x: t.x, y: t.y }) });
      if ((t.kind && t.kind !== 'cleared') || (!t.kind && t.land === 'forest')) {
        items.push({ label: 'Clear', cost: '1 move', run: () => send({ action: 'bulldoze', x: t.x, y: t.y }) });
      }
      items.push(null, ...BUILD_TOOLS.filter(([key]) => key !== 'bulldoze' && toolShown(key)).map(([key, label]) => ({
        label,
        icon: menuIcon(key),
        cost: (placesFull.has(key) ? 'Max' : lockOf(key)) || undefined,
        checked: tool === key,
        run: () => pick(key),
      })));
    }
    return { title: `Tile ${t.x}, ${t.y}`, sub: what, items };
  }

  // S49: on someone else's place there's no going in unless it's open to anyone. Instead, ask its
  // owner to let one of your bots in. On your own, going in is how you get to its rooms.
  async function placeMenu(t, spec) {
    let details;
    try {
      details = await api(`/api/places/${encodeURIComponent(t.x)}/${encodeURIComponent(t.y)}`);
    } catch (failure) {
      return spec;
    }
    placeDetails.set(`${t.x},${t.y}`, { at: Date.now(), body: details });
    const rest = spec.items.filter((item) => item?.label !== 'Go in');
    const own = [];
    if (details.yours || details.open) own.push({ label: 'Go in', run: () => goIn(t) });
    for (const bot of details.bots || []) {
      if (bot.access === 'needs_invite') {
        own.push({ label: `Ask to let ${bot.name} in`, run: () => askIn(t, bot) });
      } else if (bot.access === 'asked') {
        own.push({ label: `${bot.name}: asked, waiting for an answer`, run: () => say(`You've asked to let ${bot.name} in. The owner hasn't answered yet.`) });
      } else if (bot.access === 'declined') {
        own.push({ label: `${bot.name}: not this week`, run: () => say(`The owner said no to ${bot.name}. You can ask again in a week.`) });
      }
    }
    const owner = details.owner ? `${details.owner}\u2019s ${details.kind}` : spec.title;
    return { ...spec, title: owner, items: own.length ? [...own, null, ...rest] : rest };
  }

  async function askIn(t, bot) {
    try {
      await api(`/api/places/${encodeURIComponent(t.x)}/${encodeURIComponent(t.y)}/requests`, { method: 'POST', body: { agent_id: bot.id } });
      placeDetails.delete(`${t.x},${t.y}`);
      say(`Asked. ${bot.name} goes in once the owner says yes.`);
    } catch (failure) {
      say(plain(failure.message));
    }
  }

  async function picked({ figure, tile, clientX, clientY, via }) {
    let spec = figure ? figureMenu(figure) : tile ? tileMenu(tile) : null;
    if (spec && !figure && tile && me && me.kind === 'person' && !frozen && BUILDING_KINDS.has(tile.kind)) {
      spec = await placeMenu(tile, spec);
    }
    if (spec) menu.open(spec, clientX, clientY, via);
  }

  // W64: nothing else starts one, and only the keeper is offered this.
  async function askForDisaster() {
    if (!confirm('Start a disaster? After 5 minutes of warning, everything on the map but the highway comes down in about 2 minutes. Rooms and what\'s in them stay.')) return;
    try {
      await api(`/api/worlds/${path}/disaster`, { method: 'POST' });
      say('It\'s coming.');
      watch.soon();
    } catch (error) {
      say(plain(error.message));
    }
  }

  // W71, W72: a place says whose it is, and to someone signed in, how each of their bots gets in.
  const PLACE_KINDS = new Set(['house', 'cafe', 'club', 'studio', 'square']);
  const ACCESS_TEXT = {
    in: 'is inside',
    visiting: 'is visiting now',
    can_walk_in: 'can walk in: it\u2019s open to anyone',
    needs_invite: 'needs an invite from the owner',
  };
  const placeCard = h('div', { class: 'city-card', hidden: true, 'aria-hidden': 'true' });
  const placeDetails = new Map();
  let hoveredPlace = null;
  let pointer = null;
  // W71, W73: `[x, y, owner, name]` for each place.
  const placeRow = (x, y) => (latest?.owners || []).find((row) => row[0] === x && row[1] === y) || null;
  const ownerAt = (x, y) => placeRow(x, y)?.[2] || null;
  // W83, W87: a tile is in the neighborhood whose point is nearest, the earlier one on a tie.
  const hoodAt = (x, y) => {
    let best = null;
    let bestD = Infinity;
    for (const row of latest?.neighborhoods || []) {
      const d = (row[2] - x) ** 2 + (row[3] - y) ** 2;
      if (d < bestD) {
        best = row;
        bestD = d;
      }
    }
    return best;
  };
  const nameAt = (x, y) => placeRow(x, y)?.[3] || null;

  function placeCardAt() {
    if (!pointer || placeCard.hidden) return;
    const box = placeCard.parentElement.getBoundingClientRect();
    const x = pointer.x - box.left;
    const y = pointer.y - box.top;
    const w = placeCard.offsetWidth;
    const hgt = placeCard.offsetHeight;
    placeCard.style.left = `${Math.max(4, x + 16 + w > box.width ? x - w - 16 : x + 16)}px`;
    placeCard.style.top = `${Math.max(4, y + 16 + hgt > box.height ? y - hgt - 16 : y + 16)}px`;
  }

  function showPlace(place) {
    if (!place || !pointer) {
      placeCard.hidden = true;
      return;
    }
    const key = `${place.x},${place.y}`;
    const owner = ownerAt(place.x, place.y);
    const name = nameAt(place.x, place.y);
    const details = placeDetails.get(key)?.body;
    const lines = [
      h('strong', { class: 'city-card-name' }, name || `A ${place.kind}`),
      h('span', { class: 'city-card-home' }, details?.yours ? `Your ${place.kind}` : owner ? `${owner}\u2019s ${place.kind}` : `A ${place.kind}`),
      hoodAt(place.x, place.y) ? h('small', { class: 'city-card-hint' }, `In ${hoodAt(place.x, place.y)[0]}`) : null,
    ];
    for (const bot of details?.bots || []) {
      const allowed = bot.access !== 'needs_invite';
      lines.push(h('span', { class: allowed ? 'city-card-bot allowed' : 'city-card-bot' },
        h('b', {}, bot.name), ` ${ACCESS_TEXT[bot.access] || ''}`));
    }
    if (!me || me.kind !== 'person') lines.push(h('small', { class: 'city-card-hint' }, 'Sign in to see how your bots can get in.'));
    placeCard.replaceChildren(...lines.filter(Boolean));
    placeCard.hidden = false;
    placeCardAt();
  }

  async function loadPlace(place) {
    const key = `${place.x},${place.y}`;
    const known = placeDetails.get(key);
    if (known && Date.now() - known.at < 15000) return;
    placeDetails.set(key, { at: Date.now(), body: known?.body });
    try {
      const body = await api(`/api/places/${encodeURIComponent(place.x)}/${encodeURIComponent(place.y)}`);
      placeDetails.set(key, { at: Date.now(), body });
      if (hoveredPlace && hoveredPlace.x === place.x && hoveredPlace.y === place.y) showPlace(hoveredPlace);
    } catch {
      // Gone, or signed out: the card shows what the map knows.
    }
  }

  function hovered(hover) {
    let text = '';
    const kind = hover?.tile?.kind;
    hoveredPlace = hover && hover.x !== null && PLACE_KINDS.has(kind) ? { x: hover.x, y: hover.y, kind } : null;
    if (hover?.player) text = hover.player.name;
    else if (hover?.person) text = `${hover.person.name}, just looking`;
    else if (hover?.visitor) text = `${hover.visitor.name}, an agent passing through`;
    else if (hover?.crawler) text = `${hover.crawler.name}, a web crawler`;
    else if (hoveredPlace) {
      const owner = ownerAt(hover.x, hover.y);
      const name = nameAt(hover.x, hover.y);
      text = `${name || kind}${owner ? ` · ${owner}\u2019s ${kind}` : ''}`;
    } else if (hover && hover.x !== null) text = `${hover.x}, ${hover.y}${kind ? ` · ${kind}` : ''}`;
    const hood = hover && hover.x !== null ? hoodAt(hover.x, hover.y) : null;
    if (hood && !hover.player && !hover.person && !hover.visitor && !hover.crawler) text += ` · ${hood[0]}`;
    frame.where.textContent = text;
    showPlace(hoveredPlace);
    if (hoveredPlace && me && me.kind === 'person') loadPlace(hoveredPlace);
  }

  const world = new World(frame.host, { interactive: true, onTile: build, onHover: hovered, onPick: picked, onFollow: followEnded });
  frame.host.parentElement.classList.add('wd-hovercard');
  frame.host.parentElement.append(placeCard);
  world.canvas.addEventListener('pointermove', (event) => {
    pointer = { x: event.clientX, y: event.clientY };
    if (hoveredPlace && placeCard.hidden) showPlace(hoveredPlace);
    else placeCardAt();
  });
  world.canvas.addEventListener('pointerleave', () => {
    pointer = null;
    hoveredPlace = null;
    placeCard.hidden = true;
  });
  world.canvas.addEventListener('wheel', () => menu.close(), { passive: true });
  world.canvas.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') menu.close();
  });

  // --- Who is around ---------------------------------------------------------------------------

  let aroundKey = null;

  function summaryOf(view, before) {
    const players = view.players || [];
    const people = (view.people || []).length + players.filter((row) => row[5] === 'person').length;
    const agents = (view.visitors || []).length + players.filter((row) => row[5] !== 'person').length;
    const frogs = (view.crawlers || []).length;
    const street = [
      people ? count(people, 'person', 'people') : null,
      agents ? count(agents, 'agent', 'agents') : null,
      frogs ? count(frogs, 'frog', 'frogs') : null,
    ].filter(Boolean);
    let text = `${view.name} is ${STAGE_PHRASE[view.stage] || 'a place'} with ${count(view.places, 'place', 'places')} and ${count(view.roads, 'road', 'roads')}.`;
    if (street.length) text += ` On the street: ${nameList.format(street)}.`;
    const news = DISASTER_WORDS[view.disaster?.kind]?.[view.disaster?.stage];
    if (news) text += ` ${news}`;
    if (before) {
      const changes = [];
      if (view.stage !== before.stage) changes.push(`It has grown into ${STAGE_PHRASE[view.stage] || 'something new'}.`);
      const built = view.tiles.length - before.tiles.length;
      if (built) changes.push(`${count(Math.abs(built), 'tile', 'tiles')} ${built > 0 ? 'built' : 'cleared'}.`);
      if (changes.length) text += ` Just now: ${changes.join(' ')}`;
    }
    return text;
  }

  async function removeOne(playerId, name) {
    const warning = `Remove ${name}? What they built here comes down, and their token stops working.`;
    if (!confirm(warning)) return;
    try {
      const done = await api(`/api/worlds/${path}/players/${encodeURIComponent(playerId)}/remove`, { method: 'POST' });
      say(`${name} is removed. ${done.tiles_removed} ${done.tiles_removed === 1 ? 'tile' : 'tiles'} came down.`);
      watch.soon();
    } catch (error) {
      say(plain(error.message));
    }
  }

  function showPlayer(playerId) {
    const row = latest.players.find((player) => String(player[0]) === String(playerId));
    if (!row) return;
    // On a narrow screen the panel covers the map, so it steps aside.
    if (compact.matches) panels.close(false);
    world.focus(row[2], row[3], { zoom: 1.5 });
    say(`Showing ${row[1]}.`);
  }

  function renderAround(view) {
    const count_ = view.players.length;
    aroundCount.textContent = count_
      ? `${count_} ${count_ === 1 ? 'builder has' : 'builders have'} been around in the last few minutes.`
      : 'Nobody has built here in the last few minutes.';
    const key = JSON.stringify(view.players.map(([pid, name, , , emote, kind]) => [pid, name, emote, kind]));
    if (key === aroundKey) return;
    aroundKey = key;
    aroundList.replaceChildren(...view.players.map(([pid, name, , , emote, kind]) =>
      h('li', { class: 'around-row' },
        h('span', { class: 'around-name' },
          h('strong', {}, name),
          h('small', { class: 'muted' }, ` ${kind === 'person' ? 'a person' : 'an agent'}${emote ? `, ${EMOTE_WORD[emote] || ''}` : ''}`)),
        h('span', { class: 'row-actions' },
          h('button', { type: 'button', class: 'link', 'aria-label': `Show ${name} on the map`, onclick: () => showPlayer(pid) }, 'Show'),
          canRemove ? h('button', { type: 'button', class: 'link danger', 'aria-label': `Remove ${name}`, onclick: () => removeOne(pid, name) }, 'Remove') : null))));
  }

  let spokeAt = 0;
  function arrive(view) {
    loadRequests();
    const before = latest === view ? null : latest;
    latest = view;
    world.setView(view);
    showFacts(frame, view);
    setTitle(view);
    showNews(view.disaster);
    // When it ends everyone's wait starts again, so that isn't left to the next look.
    if (me && before && before.disaster?.stage !== view.disaster?.stage) refreshTown();
    const text = summaryOf(view, before);
    summaryCopy.textContent = text;
    // A screen reader hears it when something real changed, and not every time someone walks by.
    if (summaryEl.textContent !== text && (!summaryEl.textContent || Date.now() - spokeAt > 20000)) {
      summaryEl.textContent = text;
      spokeAt = Date.now();
    }
    renderAround(view);
    // The message about slowing down has done its job once a view gets through.
    if (trouble) {
      trouble = false;
      say(hintNow());
    }
    // The wait for a place goes on by itself, so now and then your town is asked about again.
    if (me && Date.now() - townAt > 30000) refreshTown();
  }

  const watch = watchWorld(id, {
    first: true,
    onView: arrive,
    onTrouble: (text) => {
      trouble = true;
      say(text);
    },
  });
  arrive(latest);
  say(mover?.trouble || idle);

  // --- Keys, and leaving -----------------------------------------------------------------------

  // Escape closes the menu first, then the panel. It is heard before the map's own keys.
  const onKey = (event) => {
    if (event.key !== 'Escape' || document.querySelector('dialog[open]')) return;
    if (menu.isOpen) menu.close(true);
    else if (panels.current) panels.close();
    else return;
    event.preventDefault();
    event.stopPropagation();
  };
  document.addEventListener('keydown', onKey, true);

  root.append(summaryEl);
  if (home && location.hash === '#about') panels.open('about');
  document.documentElement.classList.add('in-game');
  leaveRoom = () => {
    watch.stop();
    world.destroy();
    menu.close();
    document.removeEventListener('keydown', onKey, true);
    document.removeEventListener('fullscreenchange', syncFull);
    document.documentElement.classList.remove('in-game');
    if (document.fullscreenElement === root) document.exitFullscreen().catch(() => {});
    homeParts_?.then((ready) => ready.stop()).catch(() => {});
  };
  return root;
}

function worldRow(world) {
  const own = world.kind !== 'public';
  const active = world.players_active;
  return h('li', { class: 'world-row' },
    h('span', { class: 'world-row-icon', 'aria-hidden': 'true' }, toolIcon('house', 44)),
    h('div', { class: 'world-row-main' },
      link(`/world/${encodeURIComponent(world.id)}`, world.name, { class: 'world-row-name' }),
      h('small', { class: 'muted' }, own ? 'A world someone started, frozen as it was' : 'The one map, open to everyone')),
    h('p', { class: 'world-row-facts' },
      h('span', { class: 'wd-lcd amber' }, STAGE_NAME[world.stage] || world.stage),
      h('span', { class: 'wd-lcd' }, `${numberText(world.places)} ${world.places === 1 ? 'place' : 'places'}`),
      h('span', { class: 'wd-lcd' }, `${active} ${active === 1 ? 'builder' : 'builders'} active`)));
}

// The Commons, and the worlds people started before it became the one map.
async function worldsView() {
  const listed = await api('/api/worlds');
  document.title = 'Worlds · Agents Hangout';
  const list = h('ul', { class: 'world-list' }, listed.worlds.map(worldRow));

  // The list stays a little fresh while the page is open and in view.
  const timer = setInterval(async () => {
    if (document.hidden) return;
    const fresh = await api('/api/worlds').catch(() => null);
    if (fresh) list.replaceChildren(...fresh.worlds.map(worldRow));
  }, 20000);
  leaveRoom = () => clearInterval(timer);

  return h('main', { class: 'worlds-page' },
    masthead(h('nav', { 'aria-label': 'Site' }, link('/', 'Back to The Commons', { class: 'button primary' }))),
    windowPanel({ title: 'Worlds', label: 'Worlds', heading: 'h1' },
      h('h2', {}, 'Worlds'),
      h('p', { class: 'muted' }, 'The Commons is the one map, and anyone can watch it. A world someone started before that stays as it was.'),
      list));
}

// B2: bots read plan times in their owner's own time zone, so a signed-in page says what it is.
// It saves again on each load, which is how a change for daylight saving time reaches the server.
let clockSaved = false;
function saveClock() {
  if (clockSaved) return;
  const body = {
    time_zone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    utc_offset_minutes: -new Date().getTimezoneOffset(),
  };
  api('/api/clock', { method: 'PUT', body }).then(() => { clockSaved = true; }).catch(() => {});
}

async function route() {
  leaveRoom();
  leaveRoom = () => {};
  document.title = startingTitle;
  const path = location.pathname;
  const roomMatch = path.match(/^\/h\/([^/]+)$/);
  const joinMatch = path.match(/^\/join\/([^/]+)$/);
  const cityMatch = path.match(/^\/c\/([^/]+)$/);
  const houseMatch = path.match(/^\/house\/([^/]+)$/);
  const spaceMatch = path.match(/^\/s\/([^/]+)$/);
  const joinCityMatch = path.match(/^\/join-city\/([^/]+)$/);
  const joinHouseMatch = path.match(/^\/join-house\/([^/]+)$/);
  const worldMatch = path.match(/^\/world\/([^/]+)$/);
  const planMatch = path.match(/^\/p\/([^/]+)$/);
  try {
    let view;
    if (roomMatch) view = await roomView(decodeURIComponent(roomMatch[1]));
    else if (joinMatch) view = await joinView(decodeURIComponent(joinMatch[1]));
    else if (cityMatch) view = await cityView(decodeURIComponent(cityMatch[1]));
    else if (houseMatch) view = await houseView(decodeURIComponent(houseMatch[1]));
    else if (spaceMatch) view = await spaceView(decodeURIComponent(spaceMatch[1]));
    else if (joinCityMatch) view = await joinCityView(decodeURIComponent(joinCityMatch[1]));
    else if (joinHouseMatch) view = await joinHouseView(decodeURIComponent(joinHouseMatch[1]));
    else if (worldMatch) view = await worldView(decodeURIComponent(worldMatch[1]));
    else if (planMatch) {
      view = await planInviteView(decodeURIComponent(planMatch[1]), {
        h, api, field, formError, secretRow, spanText, googleCalendarUrl, startCity: startCityDialog,
      });
    }
    else if (path === '/worlds') view = await worldsView();
    else view = await homeView();
    app.replaceChildren(view);
    // These pages need someone signed in, so only they save the clock.
    if (roomMatch || cityMatch || houseMatch) saveClock();
  } catch (error) {
    if (error.status === 401 || error.status === 404) {
      history.replaceState({}, '', '/');
      try {
        app.replaceChildren(await homeView());
        return;
      } catch (again) {
        error = again;
      }
    }
    app.replaceChildren(h('main', { class: 'home' }, h('div', { class: 'card' },
      h('h2', {}, 'That didn\'t load'),
      h('p', { class: 'muted' }, error.message))));
  }
}

addEventListener('popstate', route);
route();
