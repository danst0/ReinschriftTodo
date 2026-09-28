/**
 * Live preview under the add line: shows which +project, @place, due: and
 * rec: tokens the server will pick out of the typed text. Mirrors the
 * patterns in app/models/todo.py; anything else stays part of the title.
 */

const PROJECT_RE = /\+(?:"((?:\\.|[^"])*)"|(\S+))/g;
const CONTEXT_RE = /@(?:"((?:\\.|[^"])*)"|(\S+))/g;
const DUE_RE = /due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?/;
const RECUR_RE = /rec:(\S+)/;

function names(text, re) {
    return Array.from(text.matchAll(re), m => (m[1] ?? m[2]).replace(/\\(.)/g, '$1'));
}

function token(className, text) {
    const span = document.createElement('span');
    span.className = `tok ${className}`;
    const b = document.createElement('b');
    b.textContent = text;
    span.appendChild(b);
    return span;
}

function formatDue(date, time, lang) {
    const d = new Date(`${date}T${time || '00:00'}`);
    if (Number.isNaN(d.getTime())) return date;
    if (d.getFullYear() === 9999) return null;
    const opts = { weekday: 'short', day: 'numeric', month: 'numeric' };
    if (time) Object.assign(opts, { hour: '2-digit', minute: '2-digit' });
    try {
        return new Intl.DateTimeFormat(lang, opts).format(d);
    } catch (e) {
        return time ? `${date} ${time}` : date;
    }
}

/**
 * @param {object} options
 * @param {string} options.language - UI language for date formatting
 * @param {boolean} options.aiOnAdd - AI rewrites the task after adding, so
 *     the default due date is not a promise the preview can make
 */
export function initAddPreview({ language = 'de', aiOnAdd = false } = {}) {
    const input = document.getElementById('add-input');
    const preview = document.getElementById('add-preview');
    if (!input || !preview) return;

    const d = preview.dataset;

    const render = () => {
        const text = input.value;
        preview.replaceChildren();
        if (!text.trim()) {
            preview.textContent = d.hint;
            return;
        }

        const parts = [];
        names(text, PROJECT_RE).forEach(n => parts.push(token('project', `+${n}`)));
        names(text, CONTEXT_RE).forEach(n => parts.push(token('context', `@${n}`)));

        const due = text.match(DUE_RE);
        let dueText = null;
        if (due) {
            dueText = formatDue(due[1], due[2], language) ?? d.sometime;
        } else if (!aiOnAdd) {
            dueText = d.today;
        }
        if (dueText) {
            const span = token('due', dueText);
            span.prepend(`${d.due} `);
            parts.push(span);
        }

        const rec = text.match(RECUR_RE);
        if (rec) parts.push(token('recurrence', `rec:${rec[1]}`));

        preview.append(`${d.recognized}:`, ...parts);
    };

    input.addEventListener('input', render);
    input.addEventListener('valuechange', render);
    // The add form resets the input after a successful add.
    input.form?.addEventListener('reset', () => setTimeout(render));
    render();
}
