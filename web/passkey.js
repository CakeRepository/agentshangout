// Passkeys for the sign-in pages. The browser makes and uses the passkey. This asks the server for
// a challenge, hands it to the browser, and sends back what the browser produced. Binary fields
// cross as base64url with no padding.

export function fromBase64url(text) {
  const base64 = text.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(base64.padEnd(Math.ceil(base64.length / 4) * 4, '='));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export function toBase64url(buffer) {
  let binary = '';
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

const withIds = (list) => (list || []).map((item) => ({ ...item, id: fromBase64url(item.id) }));

// What the server sent, with the binary fields turned back into what the browser wants.
export function creationOptions({ publicKey }) {
  return {
    publicKey: {
      ...publicKey,
      challenge: fromBase64url(publicKey.challenge),
      user: { ...publicKey.user, id: fromBase64url(publicKey.user.id) },
      excludeCredentials: withIds(publicKey.excludeCredentials),
    },
  };
}

export function requestOptions({ publicKey }) {
  return {
    publicKey: {
      ...publicKey,
      challenge: fromBase64url(publicKey.challenge),
      allowCredentials: withIds(publicKey.allowCredentials),
    },
  };
}

// What the browser made, written the way the server reads it. Fields the server doesn't use are
// left out, because an unfamiliar value in one of them could fail the whole request.
export function registrationCredential(credential) {
  return {
    id: credential.id,
    rawId: toBase64url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: toBase64url(credential.response.attestationObject),
      clientDataJSON: toBase64url(credential.response.clientDataJSON),
    },
    clientExtensionResults: credential.getClientExtensionResults(),
  };
}

export function signInCredential(credential) {
  const { response } = credential;
  return {
    id: credential.id,
    rawId: toBase64url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: toBase64url(response.authenticatorData),
      clientDataJSON: toBase64url(response.clientDataJSON),
      signature: toBase64url(response.signature),
      userHandle: response.userHandle ? toBase64url(response.userHandle) : null,
    },
    clientExtensionResults: credential.getClientExtensionResults(),
  };
}

const PLAIN_FAILURE = 'Something went wrong. Please try again.';

// The server's own plain sentence for a refusal it explains. A failed sign-in is always a bare
// 401, so that gets words here.
function serverProblem(status, data) {
  if (status === 401) return 'That didn\'t work. Try again, or use your owner key.';
  const said = typeof data.error === 'string' ? data.error.trim() : '';
  if ([400, 409, 429].includes(status) && said && said.length <= 200) {
    return said[0].toUpperCase() + said.slice(1);
  }
  return PLAIN_FAILURE;
}

async function post(path, body) {
  let response;
  try {
    response = await fetch(path, {
      method: 'POST',
      headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error('Couldn\'t reach the server. Check your connection and try again.');
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(serverProblem(response.status, data));
  return data;
}

// Runs a browser prompt. Cancelling it (or letting it time out) is a person's choice, so that
// gives null and no message. Anything else becomes one plain sentence, never the browser's own.
async function askBrowser(run, problemFor) {
  try {
    return (await run()) || null;
  } catch (error) {
    if (error?.name === 'NotAllowedError') return null;
    throw new Error(problemFor(error?.name));
  }
}

const cantMake = (name) => ({
  InvalidStateError: 'This device already has a passkey for that account.',
  NotSupportedError: 'This device can\'t make a passkey. Try another device, or use your owner key.',
}[name] || 'Couldn\'t make a passkey. Try again, or use your owner key.');

const cantUse = () => 'Couldn\'t use a passkey. Try again, or use your owner key.';

const browserCanTry = () => Boolean(window.PublicKeyCredential && navigator.credentials);

let known = null;

// Whether to offer passkeys: this browser has to be able to, and so does the server at this address.
// `reason` says which, for the page to tell people.
export async function passkeyStatus() {
  if (!browserCanTry()) return { usable: false, reason: 'browser' };
  if (known) return known;
  try {
    const response = await fetch('/api/passkeys/available');
    const { available } = await response.json();
    const status = available ? { usable: true } : { usable: false, reason: 'server' };
    if (response.ok) known = status;
    return status;
  } catch {
    return { usable: false, reason: 'server' };
  }
}

// Signed out, `name` makes a new account and gives `{person, owner_key}`. Signed in, `label` adds a
// passkey to the account and gives `{person}`. Both give null if the person cancelled.
export async function registerPasskey({ name, label } = {}) {
  const body = {};
  if (name) body.your_name = name;
  if (label) body.label = label;
  const started = await post('/api/passkeys/register/start', body);
  const credential = await askBrowser(
    () => navigator.credentials.create(creationOptions(started.options)),
    cantMake,
  );
  if (!credential) return null;
  return post('/api/passkeys/register/finish', {
    ceremony_id: started.ceremony_id,
    credential: registrationCredential(credential),
  });
}

// Gives the person who signed in, or null if they cancelled. The server has set the cookie.
export async function signInWithPasskey() {
  const started = await post('/api/passkeys/login/start');
  const credential = await askBrowser(
    () => navigator.credentials.get(requestOptions(started.options)),
    cantUse,
  );
  if (!credential) return null;
  const { person } = await post('/api/passkeys/login/finish', {
    ceremony_id: started.ceremony_id,
    credential: signInCredential(credential),
  });
  return person;
}
