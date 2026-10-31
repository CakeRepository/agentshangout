// The page a plan's link opens. A friend sees the plan and answers it, with no account if they have
// none (F12). It shows only what the server's preview holds, and builds everything with text nodes.

const AFTER = {
  yes: 'You\'re in.',
  maybe: 'You\'re down as a maybe.',
  no: 'Thanks for letting them know.',
};

export async function planInviteView(code, ui) {
  const { h, api, field, formError, secretRow, whenText, googleCalendarUrl, startCity } = ui;
  const path = `/api/plan-invites/${encodeURIComponent(code)}`;
  const root = h('main', { class: 'home' });

  let plan;
  try {
    plan = await api(path);
  } catch (failure) {
    if (failure.status !== 404) throw failure;
    root.append(h('div', { class: 'card' },
      h('h2', {}, 'This link doesn\'t work any more'),
      h('p', {}, 'The plan may be over or cancelled, or the link may have been replaced. Ask your friend for a new one.')));
    return root;
  }
  document.title = `${plan.title} · Agents Hangout`;

  const details = (shown) => [
    h('h2', {}, shown.title),
    h('p', { class: 'muted' }, [whenText(shown.starts_at), shown.location].filter(Boolean).join(' · ')),
    h('p', { class: `pill ${shown.on_calendar ? 'on' : ''}` }, shown.on_calendar
      ? 'On the calendar'
      : `${shown.yes} of ${shown.yes_needed} yeses so far`),
  ];

  async function showAsking(shown) {
    const me = await api('/api/me').catch(() => null);
    const error = formError();
    const name = me ? null : h('input', { name: 'your_name', required: true, maxLength: 60, autocomplete: 'given-name' });
    const buttons = ['yes', 'maybe', 'no'].map((response) => h('button', {
      type: 'submit',
      class: shown.your_answer === response ? 'chosen' : (response === 'yes' ? 'primary' : 'ghost'),
      value: response,
    }, response[0].toUpperCase() + response.slice(1)));
    const form = h('form', {
      class: 'card-form',
      onsubmit: async (event) => {
        event.preventDefault();
        error.hidden = true;
        buttons.forEach((button) => { button.disabled = true; });
        try {
          const answered = await api(`${path}/answer`, {
            method: 'POST',
            body: { response: event.submitter.value, your_name: name?.value },
          });
          await showAnswered(answered.plan, answered.owner_key);
        } catch (failure) {
          error.textContent = failure.message;
          error.hidden = false;
        } finally {
          buttons.forEach((button) => { button.disabled = false; });
        }
      },
    },
      name && field('Your name', name, 'So your friends know who answered. You don\'t need an account.'),
      error,
      h('div', { class: 'event-actions' }, buttons));
    root.replaceChildren(h('div', { class: 'card' },
      h('p', { class: 'muted' }, `${shown.invited_by} invited you to`),
      ...details(shown),
      form));
  }

  async function showAnswered(shown, ownerKey) {
    const me = await api('/api/me').catch(() => null);
    const going = shown.your_answer !== 'no';
    root.replaceChildren(h('div', { class: 'card' },
      h('p', { class: 'muted' }, AFTER[shown.your_answer]),
      ...details(shown),
      h('div', { class: 'event-actions' },
        going && h('a', { href: googleCalendarUrl(shown), target: '_blank', rel: 'noopener', class: 'link' }, 'Add to Google Calendar'),
        going && h('a', { href: `${path}/plan.ics`, class: 'link' }, 'Download for your calendar'),
        h('button', { type: 'button', class: 'link', onclick: () => showAsking(shown) }, 'Change my answer')),
      ownerKey && h('details', { class: 'quiet' },
        h('summary', {}, 'Your key, for getting back in on another device'),
        h('p', { class: 'muted' }, 'We show this once. This browser stays signed in without it.'),
        secretRow(ownerKey))),
    me?.may_start_city ? h('div', { class: 'card' },
      h('h2', {}, 'Let your agent answer next time'),
      h('p', {}, 'Start a city for your own friends and bring your agent. Your agents can find a time together, and you only have to say yes.'),
      h('button', { type: 'button', class: 'primary', onclick: () => startCity(me) }, 'Start a city')) : null);
  }

  if (plan.your_answer) await showAnswered(plan);
  else await showAsking(plan);
  return root;
}
