/**
 * Live title autocomplete for the Add and Edit inputs.
 *
 * Fetches the de-duplicated, frequency-sorted title list from
 * `/api/title-suggestions` once per page load. While typing in a bound
 * input (>= 2 chars), shows up to 8 substring matches in a dropdown
 * anchored beneath the input. ArrowDown/ArrowUp navigate, Enter selects,
 * Escape closes (without consuming the host form's Enter/Escape when the
 * dropdown is hidden). Selecting a row replaces the input value entirely.
 */

const ENDPOINT = '/api/title-suggestions';
const MIN_CHARS = 2;
const MAX_RESULTS = 8;

let cachedTitles = null;
let inflight = null;

async function fetchTitles(force = false) {
    if (!force && cachedTitles) return cachedTitles;
    if (inflight) return inflight;
    inflight = (async () => {
        try {
            const res = await fetch(ENDPOINT, { credentials: 'same-origin' });
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            cachedTitles = Array.isArray(data.titles) ? data.titles : [];
            return cachedTitles;
        } catch (err) {
            console.warn('title-suggestions fetch failed', err);
            cachedTitles = cachedTitles || [];
            return cachedTitles;
        } finally {
            inflight = null;
        }
    })();
    return inflight;
}

/** Drop the in-memory cache so the next bound input refetches. */
export function invalidateTitleCache() {
    cachedTitles = null;
}

function filterTitles(query, titles) {
    const q = query.trim().toLowerCase();
    if (q.length < MIN_CHARS) return [];
    const out = [];
    for (const title of titles) {
        if (title.toLowerCase().includes(q) && title.toLowerCase() !== q) {
            out.push(title);
            if (out.length >= MAX_RESULTS) break;
        }
    }
    return out;
}

function attachDropdown(input) {
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
        input.value = currentMatches[idx];
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
        matches.forEach((title, i) => {
            const li = document.createElement('li');
            li.className = 'autocomplete-item';
            li.setAttribute('role', 'option');
            li.textContent = title;
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
        const titles = await fetchTitles();
        render(filterTitles(input.value, titles));
    });

    input.addEventListener('focus', async () => {
        if (input.value.trim().length >= MIN_CHARS) {
            const titles = await fetchTitles();
            render(filterTitles(input.value, titles));
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
 */
export function initTitleAutocomplete({ inputSelectors = [], enabled = true } = {}) {
    if (!enabled) return;
    for (const sel of inputSelectors) {
        const el = document.querySelector(sel);
        if (el && el.tagName === 'INPUT') {
            attachDropdown(el);
        }
    }
    // Warm the cache so first keystroke is instant.
    fetchTitles().catch(() => {});
}
