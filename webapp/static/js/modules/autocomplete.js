/**
 * Live title autocomplete for the Add and Edit inputs.
 *
 * Fetches the de-duplicated, frequency-sorted title list from
 * `/api/title-suggestions` once per page load. While typing in a bound
 * input (>= 2 chars), shows up to 8 substring matches in a dropdown
 * anchored beneath the input. ArrowDown/ArrowUp navigate, Enter selects,
 * Escape closes (without consuming the host form's Enter/Escape when the
 * dropdown is hidden). Selecting a row replaces the input value entirely.
 *
 * Inputs bound with `allowDuplicate` additionally show a copy button per
 * suggestion that duplicates the matched task (with its metadata) as a
 * new open task via `/api/duplicate` (issue #6).
 */

import { fetchWithCsrf } from './api.js';

const ENDPOINT = '/api/title-suggestions';
const MIN_CHARS = 2;
const MAX_RESULTS = 8;

let cachedSuggestions = null; // [{title, marker}]
let inflight = null;
let translations = {};
let onDuplicated = null;

async function fetchSuggestions(force = false) {
    if (!force && cachedSuggestions) return cachedSuggestions;
    if (inflight) return inflight;
    inflight = (async () => {
        try {
            const res = await fetch(ENDPOINT, { credentials: 'same-origin' });
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            if (Array.isArray(data.items)) {
                cachedSuggestions = data.items;
            } else if (Array.isArray(data.titles)) {
                cachedSuggestions = data.titles.map(title => ({ title, marker: null }));
            } else {
                cachedSuggestions = [];
            }
            return cachedSuggestions;
        } catch (err) {
            console.warn('title-suggestions fetch failed', err);
            cachedSuggestions = cachedSuggestions || [];
            return cachedSuggestions;
        } finally {
            inflight = null;
        }
    })();
    return inflight;
}

/** Drop the in-memory cache so the next bound input refetches. */
export function invalidateTitleCache() {
    cachedSuggestions = null;
}

function filterSuggestions(query, suggestions) {
    const q = query.trim().toLowerCase();
    if (q.length < MIN_CHARS) return [];
    const out = [];
    for (const suggestion of suggestions) {
        const title = suggestion.title.toLowerCase();
        if (title.includes(q) && title !== q) {
            out.push(suggestion);
            if (out.length >= MAX_RESULTS) break;
        }
    }
    return out;
}

async function duplicateTask(marker) {
    try {
        const res = await fetchWithCsrf('/api/duplicate', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ marker })
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        invalidateTitleCache();
        if (onDuplicated) onDuplicated(data.marker);
    } catch (err) {
        console.error('Duplicate failed', err);
    }
}

function attachDropdown(input, { allowDuplicate = false } = {}) {
    // Position-fixed so we don't interfere with existing flex/grid layouts.
    const dropdown = document.createElement('ul');
    dropdown.className = 'autocomplete-dropdown';
    dropdown.setAttribute('role', 'listbox');
    dropdown.hidden = true;
    document.body.appendChild(dropdown);

    const positionDropdown = () => {
        const rect = input.getBoundingClientRect();
        dropdown.style.left = `${rect.left}px`;
        dropdown.style.top = `${rect.bottom}px`;
        dropdown.style.width = `${rect.width}px`;
    };

    let activeIndex = -1;
    let currentMatches = [];

    const close = () => {
        dropdown.hidden = true;
        dropdown.innerHTML = '';
        activeIndex = -1;
        currentMatches = [];
    };

    const setActive = (idx) => {
        const items = dropdown.querySelectorAll('li');
        items.forEach((li, i) => {
            li.classList.toggle('active', i === idx);
            if (i === idx) li.scrollIntoView({ block: 'nearest' });
        });
        activeIndex = idx;
    };

    const accept = (idx) => {
        if (idx < 0 || idx >= currentMatches.length) return;
        input.value = currentMatches[idx].title;
        input.focus();
        // Move caret to end
        const len = input.value.length;
        input.setSelectionRange(len, len);
        close();
    };

    const render = (matches) => {
        dropdown.innerHTML = '';
        currentMatches = matches;
        if (matches.length === 0) {
            close();
            return;
        }
        matches.forEach((match, i) => {
            const li = document.createElement('li');
            li.className = 'autocomplete-item';
            li.setAttribute('role', 'option');

            const titleSpan = document.createElement('span');
            titleSpan.className = 'autocomplete-title';
            titleSpan.textContent = match.title;
            li.appendChild(titleSpan);

            if (allowDuplicate && match.marker) {
                const copyBtn = document.createElement('button');
                copyBtn.type = 'button';
                copyBtn.className = 'autocomplete-copy-btn';
                copyBtn.title = translations.duplicate || 'Duplicate';
                copyBtn.setAttribute('aria-label', copyBtn.title);
                copyBtn.innerHTML =
                    '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" ' +
                    'stroke="currentColor" stroke-width="2" stroke-linecap="round" ' +
                    'stroke-linejoin="round" aria-hidden="true">' +
                    '<rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>' +
                    '<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>' +
                    '</svg>';
                copyBtn.addEventListener('mousedown', (e) => {
                    // mousedown so we beat the input's blur
                    e.preventDefault();
                    e.stopPropagation();
                    input.value = '';
                    close();
                    duplicateTask(match.marker);
                });
                li.appendChild(copyBtn);
            }

            li.addEventListener('mousedown', (e) => {
                // mousedown so we beat the input's blur
                e.preventDefault();
                accept(i);
            });
            li.addEventListener('mouseenter', () => setActive(i));
            dropdown.appendChild(li);
        });
        positionDropdown();
        dropdown.hidden = false;
        activeIndex = -1;
    };

    window.addEventListener('scroll', () => {
        if (!dropdown.hidden) positionDropdown();
    }, true);
    window.addEventListener('resize', () => {
        if (!dropdown.hidden) positionDropdown();
    });

    input.addEventListener('input', async () => {
        const suggestions = await fetchSuggestions();
        render(filterSuggestions(input.value, suggestions));
    });

    input.addEventListener('focus', async () => {
        if (input.value.trim().length >= MIN_CHARS) {
            const suggestions = await fetchSuggestions();
            render(filterSuggestions(input.value, suggestions));
        }
    });

    input.addEventListener('blur', () => {
        // Slight delay so click handlers on dropdown items can fire first.
        setTimeout(close, 100);
    });

    input.addEventListener('keydown', (e) => {
        if (dropdown.hidden) return;
        switch (e.key) {
            case 'ArrowDown':
                e.preventDefault();
                setActive((activeIndex + 1) % currentMatches.length);
                break;
            case 'ArrowUp':
                e.preventDefault();
                setActive(activeIndex <= 0 ? currentMatches.length - 1 : activeIndex - 1);
                break;
            case 'Enter':
                if (activeIndex >= 0) {
                    e.preventDefault();
                    e.stopPropagation();
                    accept(activeIndex);
                }
                break;
            case 'Escape':
                e.preventDefault();
                e.stopPropagation();
                close();
                break;
        }
    });
}

/**
 * Bind autocomplete to one or more inputs.
 *
 * @param {object} opts
 * @param {string[]} opts.inputSelectors - CSS selectors. Each selector may
 *   match an element that exists at init time, or one inside a modal that
 *   is created later (we re-resolve on init only — modal inputs in this
 *   codebase live in the DOM from the start, just hidden, so a single bind
 *   is enough).
 * @param {boolean} [opts.enabled=true] - Disable to no-op (e.g. user pref).
 * @param {string[]} [opts.duplicateSelectors=[]] - Subset of inputs whose
 *   suggestions get a copy button that duplicates the matched task.
 * @param {object} [opts.translations={}] - Localized strings (duplicate).
 * @param {function} [opts.onDuplicated] - Called with the new marker after
 *   a successful duplicate (reload + highlight).
 */
export function initTitleAutocomplete({
    inputSelectors = [],
    enabled = true,
    duplicateSelectors = [],
    translations: t = {},
    onDuplicated: duplicatedCallback = null
} = {}) {
    if (!enabled) return;
    translations = t;
    onDuplicated = duplicatedCallback;
    for (const sel of inputSelectors) {
        const el = document.querySelector(sel);
        if (el && el.tagName === 'INPUT') {
            attachDropdown(el, { allowDuplicate: duplicateSelectors.includes(sel) });
        }
    }
    // Warm the cache so first keystroke is instant.
    fetchSuggestions().catch(() => {});
}
