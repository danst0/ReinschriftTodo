// Pure JS port of the Reinschrift markdown format.
//
// Source of truth: core/src/parser.rs, core/src/renderer.rs,
// core/src/util.rs, core/src/todo.rs (toggle_todos / next_due_date).
// No GJS imports here — this module must run under `gjs -m` for tests.

// ---------------------------------------------------------------- regexes

const LINK_RE = /\[\[([^\]]+)\]\]/;
const PROJECT_RE = /\+(?:"((?:\\.|[^"])*)"|([^\s]+))/;
const CONTEXT_RE = /@(?:"((?:\\.|[^"])*)"|([^\s]+))/;
const DUE_RE = /due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?/;
const ID_RE = /\^([A-Za-z0-9]+)/g;
const COMPLETION_RE = /\s✅\s\d{4}-\d{2}-\d{2}/;
const RECUR_RE = /rec:([^\s]+)/;
const MYDAY_RE = /myday:(\d{4}-\d{2}-\d{2})/;
const NOTE_RE = /~note:"((?:\\.|[^"])*)"/;
const MYDAY_STRIP_RE = /\s*myday:\d{4}-\d{2}-\d{2}/;

// Markers that delimit fields in a todo line (space-prefixed variants only).
export const FIELD_MARKERS = [
    ' +', ' @', ' due:', ' myday:', ' rec:', ' [[', ' ~note:', ' ^', ' ✅',
];

const MARKER_LEN = 8;
const ALPHABET = '0123456789abcdefghijklmnopqrstuvwxyz';

// ---------------------------------------------------------------- helpers

function pad2(n) {
    return String(n).padStart(2, '0');
}

function startOfToday() {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

export function todayString() {
    const d = new Date();
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function dateString(d) {
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

export function formatDue(d) {
    return `${dateString(d)}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

export function escapeNote(note) {
    return note
        .replace(/\\/g, '\\\\')
        .replace(/"/g, '\\"')
        .replace(/\r/g, '\\r')
        .replace(/\n/g, '\\n');
}

export function unescapeNote(input) {
    let out = '';
    for (let i = 0; i < input.length; i++) {
        const c = input[i];
        if (c !== '\\') {
            out += c;
            continue;
        }
        i++;
        if (i >= input.length) {
            out += '\\';
            break;
        }
        const n = input[i];
        if (n === 'n')
            out += '\n';
        else if (n === 'r')
            out += '\r';
        else if (n === '"')
            out += '"';
        else if (n === '\\')
            out += '\\';
        else
            out += n;
    }
    return out;
}

export function normalizeToken(value) {
    if (value == null)
        return null;
    const trimmed = value.trim();
    let out = '';
    let found = false;
    for (const c of trimmed) {
        if (!found && (c === '+' || c === '@'))
            continue;
        found = true;
        out += c;
    }
    out = out.trim();
    return out === '' ? null : out;
}

export function normalizeReference(value) {
    if (value == null)
        return null;
    const trimmed = value.trim();
    return trimmed === '' ? null : trimmed;
}

export function normalizeNote(value) {
    if (value == null)
        return null;
    const trimmed = value.trim();
    return trimmed === '' ? null : trimmed;
}

// ---------------------------------------------------------------- markers

export function encodeBase36(value) {
    if (value === 0)
        return '0';
    let out = '';
    while (value > 0) {
        out = ALPHABET[value % 36] + out;
        value = Math.floor(value / 36);
    }
    return out;
}

export function generateMarker() {
    const value = Math.floor(Math.random() * 36 ** MARKER_LEN);
    return encodeBase36(value).padStart(MARKER_LEN, '0');
}

/**
 * Locate the `^id` that identifies a todo: the last whitespace-delimited one.
 * Returns its character offset and the id, or null.
 */
export function markerMatch(text) {
    const re = new RegExp(ID_RE.source, 'g');
    let fallback = null;
    let standalone = null;
    let m;
    while ((m = re.exec(text)) !== null) {
        const start = m.index;
        const isStandalone = start === 0 || /\s/.test(text[start - 1]);
        const entry = {index: start, id: m[1]};
        if (isStandalone)
            standalone = entry;
        else
            fallback = entry;
    }
    return standalone ?? fallback;
}

export function markerOf(text) {
    const m = markerMatch(text);
    return m ? m.id : null;
}

export function findLineByMarker(lines, marker) {
    const own = lines.findIndex(line => markerOf(line) === marker);
    if (own !== -1)
        return own;
    const needle = `^${marker}`;
    return lines.findIndex(line => line.split(/\s+/).includes(needle));
}

export function uniqueMarker(lines, pending) {
    let candidate = generateMarker();
    for (let i = 0; i < 8; i++) {
        if (findLineByMarker(lines, candidate) === -1 &&
            findLineByMarker(pending, candidate) === -1)
            break;
        candidate = generateMarker();
    }
    return candidate;
}

// ---------------------------------------------------------------- parsing

function captureAllTokens(regex, text) {
    const re = new RegExp(regex.source, 'g');
    const out = [];
    let m;
    while ((m = re.exec(text)) !== null) {
        let s;
        if (m[1] !== undefined)
            s = unescapeNote(m[1]);
        else
            s = m[2];
        s = s.trim();
        if (s !== '')
            out.push(s);
    }
    return out;
}

function captureToken(regex, text) {
    const m = regex.exec(text);
    return m && m[1] !== undefined ? m[1].trim() : null;
}

export function parseDue(text) {
    const m = DUE_RE.exec(text);
    if (!m)
        return null;
    const [, y, mo, d] = m[1].match(/(\d{4})-(\d{2})-(\d{2})/);
    let hh = 0;
    let mm = 0;
    if (m[2]) {
        hh = Number(m[2].slice(0, 2));
        mm = Number(m[2].slice(3, 5));
    }
    return new Date(Number(y), Number(mo) - 1, Number(d), hh, mm);
}

export function parseMyday(text) {
    const m = MYDAY_RE.exec(text);
    if (!m)
        return null;
    const [, y, mo, d] = m[1].match(/(\d{4})-(\d{2})-(\d{2})/);
    return new Date(Number(y), Number(mo) - 1, Number(d));
}

/** Strip leading `+project` / `@context` tokens so the title begins with real text. */
function stripLeadingMarkers(text) {
    let cleaned = text.replace(/^\s+/, '');
    for (;;) {
        const first = cleaned[0];
        let re = null;
        if (first === '+')
            re = PROJECT_RE;
        else if (first === '@')
            re = CONTEXT_RE;
        if (!re)
            break;
        const m = re.exec(cleaned);
        if (!m || m.index !== 0)
            break;
        const after = cleaned.slice(m[0].length);
        if (after === '' || !/\s/.test(after[0]))
            break;
        const afterTrimmed = after.replace(/^\s+/, '');
        if (afterTrimmed === '')
            break;
        cleaned = afterTrimmed;
    }
    return cleaned;
}

/** Extract the title from a todo line (everything before field markers). */
export function extractTitle(rest) {
    const cleanedRest = stripLeadingMarkers(rest);

    let cut = cleanedRest.length;
    for (const marker of FIELD_MARKERS) {
        const idx = cleanedRest.indexOf(marker);
        if (idx !== -1 && idx < cut)
            cut = idx;
    }

    const raw = cleanedRest.slice(0, cut);
    const cleaned = raw.trim();
    return cleaned === '' ? cleanedRest.trim() : cleaned;
}

/** Parse a markdown line into a todo item, or null when it is not one. */
export function parseLine(line, lineIndex) {
    const trimmed = line.replace(/^\s+/, '');
    let done = null;
    let rest = null;
    for (const [prefix, state] of [['- [x]', true], ['- [X]', true], ['- [ ]', false]]) {
        if (trimmed.startsWith(prefix)) {
            done = state;
            rest = trimmed.slice(prefix.length).trim();
            break;
        }
    }
    if (rest === null)
        return null;

    const title = extractTitle(rest);
    const restWithoutNote = rest.replace(NOTE_RE, '');
    const projects = captureAllTokens(PROJECT_RE, restWithoutNote);
    const contexts = captureAllTokens(CONTEXT_RE, restWithoutNote);
    const due = parseDue(rest);
    const myday = parseMyday(rest);
    const recurrence = captureToken(RECUR_RE, rest);
    const reference = captureToken(LINK_RE, rest);
    const marker = markerOf(rest);
    const note = normalizeNote(
        captureToken(NOTE_RE, rest) !== null
            ? unescapeNote(NOTE_RE.exec(rest)[1])
            : null);

    return {
        key: {lineIndex, marker},
        title,
        projects,
        contexts,
        due,
        myday,
        reference,
        recurrence,
        note,
        done,
    };
}

// ---------------------------------------------------------------- rendering

function renderTagged(prefix, name) {
    const needsQuote = /[\s"\\]/.test(name);
    return needsQuote
        ? `${prefix}"${escapeNote(name)}"`
        : `${prefix}${name}`;
}

/** Render a todo item to a markdown line. */
export function renderLine(item) {
    const title = item.title.trim();
    if (title === '')
        throw new Error('Title must not be empty');

    const checkbox = item.done ? '- [x]' : '- [ ]';
    const parts = [`${checkbox} ${title}`];

    for (const project of item.projects ?? []) {
        const n = normalizeToken(project);
        if (n !== null)
            parts.push(renderTagged('+', n));
    }
    for (const context of item.contexts ?? []) {
        const n = normalizeToken(context);
        if (n !== null)
            parts.push(renderTagged('@', n));
    }
    if (item.due)
        parts.push(`due:${formatDue(item.due)}`);

    // Stale "my day" dates (before today) are dropped on re-render.
    if (item.myday && item.myday.getTime() >= startOfToday().getTime())
        parts.push(`myday:${dateString(item.myday)}`);

    const recur = normalizeToken(item.recurrence);
    if (recur !== null)
        parts.push(`rec:${recur}`);

    const reference = normalizeReference(item.reference);
    if (reference !== null)
        parts.push(`[[${reference}]]`);

    const note = normalizeNote(item.note);
    if (note !== null)
        parts.push(`~note:"${escapeNote(note)}"`);

    if (item.done)
        parts.push(`✅ ${todayString()}`);

    const marker = item.key.marker && item.key.marker !== ''
        ? item.key.marker
        : generateMarker();
    parts.push(`^${marker}`);

    return parts.join(' ');
}

/** Insert a due segment into a line at the appropriate position. */
function insertDueSegment(line, segment) {
    let insertAt = line.length;
    for (const marker of FIELD_MARKERS) {
        const idx = line.indexOf(marker);
        if (idx !== -1 && idx < insertAt)
            insertAt = idx;
    }

    const head = line.slice(0, insertAt);
    const tail = line.slice(insertAt);
    const needsSpace = head !== '' && !head.endsWith(' ');
    return needsSpace ? `${head} ${segment}${tail}` : `${head}${segment}${tail}`;
}

export function rewriteDue(line, newDue) {
    const segment = `due:${formatDue(newDue)}`;
    if (DUE_RE.test(line))
        return line.replace(DUE_RE, segment);
    return insertDueSegment(line, segment);
}

/** Apply or remove the completion marker (✅ date). */
export function applyCompletionMarker(line, done) {
    if (!done)
        return line.replace(COMPLETION_RE, '');

    const m = COMPLETION_RE.exec(line);
    if (m) {
        const idm = markerMatch(line);
        if (idm && m.index > idm.index) {
            const doneStr = `${m[0].trimStart()} `;
            let s = line.slice(0, m.index) + line.slice(m.index + m[0].length);
            const newIdm = markerMatch(s);
            if (newIdm) {
                s = s.slice(0, newIdm.index) + doneStr + s.slice(newIdm.index);
                return s;
            }
        }
        return line;
    }

    const today = todayString();
    const idm = markerMatch(line);
    if (idm)
        return line.slice(0, idm.index) + `✅ ${today} ` + line.slice(idm.index);
    return `${line} ✅ ${today}`;
}

/** Rewrite a line to toggle its done state. */
export function rewriteLine(line, done) {
    let updated = line;
    const hasChecked = updated.includes('- [x]') || updated.includes('- [X]');
    const hasUnchecked = updated.includes('- [ ]');

    if (done) {
        if (!hasChecked) {
            if (hasUnchecked)
                updated = updated.replace('- [ ]', '- [x]');
            else
                throw new Error('Line contains no checkbox');
        } else if (updated.includes('- [X]')) {
            updated = updated.replace('- [X]', '- [x]');
        }
    } else if (hasChecked) {
        updated = updated.replace('- [x]', '- [ ]').replace('- [X]', '- [ ]');
    } else if (!hasUnchecked) {
        throw new Error('Line contains no checkbox');
    }

    return applyCompletionMarker(updated, done);
}

// ---------------------------------------------------------------- dates

function addDays(date, n) {
    return new Date(date.getFullYear(), date.getMonth(), date.getDate() + n,
        date.getHours(), date.getMinutes());
}

/** Add months to a date, handling month-end edge cases. */
export function addMonths(date, months) {
    const total = date.getFullYear() * 12 + date.getMonth() + months;
    const newYear = Math.floor(total / 12);
    const newMonth0 = ((total % 12) + 12) % 12;

    const lastDay = new Date(newYear, newMonth0 + 1, 0).getDate();
    const day = Math.min(date.getDate(), lastDay);
    return new Date(newYear, newMonth0, day, date.getHours(), date.getMinutes());
}

/** Next occurrence of a recurrence rule strictly after today. */
export function nextDueDate(currentDue, rule) {
    const hours = currentDue ? currentDue.getHours() : 0;
    const minutes = currentDue ? currentDue.getMinutes() : 0;
    let next = currentDue
        ? new Date(currentDue.getFullYear(), currentDue.getMonth(), currentDue.getDate())
        : startOfToday();
    const today = startOfToday();
    const r = (rule ?? '').toLowerCase();

    for (;;) {
        if (r === 'daily')
            next = addDays(next, 1);
        else if (r === 'weekly')
            next = addDays(next, 7);
        else if (r === 'monthly')
            next = addMonths(next, 1);
        else
            return null;

        if (next.getTime() > today.getTime())
            break;
    }
    return new Date(next.getFullYear(), next.getMonth(), next.getDate(), hours, minutes);
}

// ---------------------------------------------------------------- file ops

export function splitLines(content) {
    const lines = content.split('\n');
    if (content !== '' && lines[lines.length - 1] === '')
        lines.pop();
    return content === '' ? [] : lines;
}

export function joinLines(lines, hadTrailingNewline) {
    let output = lines.join('\n');
    if (hadTrailingNewline)
        output += '\n';
    return output;
}

function resolveKey(lines, key) {
    if (key.marker != null) {
        const idx = findLineByMarker(lines, key.marker);
        if (idx !== -1)
            return idx;
    }
    if (key.lineIndex != null && key.lineIndex < lines.length)
        return key.lineIndex;
    throw new Error('Could not find To-do in file');
}

/**
 * Toggle one or more todos in a single pass over the lines — mirrors
 * core/src/todo.rs `toggle_todos`, including recurrence handling:
 * completing a recurring task reschedules an overdue occurrence to today
 * first and spawns the next occurrence with a fresh marker.
 *
 * Returns the new line array.
 */
export function toggleInLines(lines, keys, done) {
    const now = new Date();
    const spawned = [];

    for (const key of keys) {
        let index;
        try {
            index = resolveKey(lines, key);
        } catch {
            continue;
        }
        const item = parseLine(lines[index], index);

        let updated = lines[index];
        const spawnsRecurrence = done && item && item.recurrence && !item.done;
        if (spawnsRecurrence && item.due && item.due.getTime() < now.getTime()) {
            const todayDue = new Date(
                now.getFullYear(), now.getMonth(), now.getDate(),
                item.due.getHours(), item.due.getMinutes());
            updated = rewriteDue(updated, todayDue);
        }

        let toggled;
        try {
            toggled = rewriteLine(updated, done);
        } catch {
            continue;
        }
        lines[index] = toggled;

        if (spawnsRecurrence) {
            const nextDue = nextDueDate(item.due, item.recurrence);
            if (nextDue) {
                const nextItem = {
                    ...item,
                    key: {lineIndex: 0, marker: uniqueMarker(lines, spawned)},
                    done: false,
                    due: nextDue,
                    myday: null,
                };
                spawned.push(renderLine(nextItem));
            }
        }
    }

    // Append next occurrences only after all index-based edits.
    if (spawned.length > 0) {
        let insertIndex = lines.findIndex(l => l.trim() === '---');
        if (insertIndex === -1)
            insertIndex = lines.length;
        lines.splice(insertIndex, 0, ...spawned);
    }
    return lines;
}
