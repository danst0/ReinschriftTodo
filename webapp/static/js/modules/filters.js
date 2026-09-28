/**
 * View tabs and the filter panel (due range, projects, places, done).
 *
 * Every change reloads the list as a partial and puts the state into the
 * URL; the server stores it in the settings, so the filter survives reloads
 * and other devices pick it up.
 */

const FILTER_PARAMS = [
    'filter_set', 'filter_reset', 'filter_due', 'filter_undated',
    'filter_project', 'filter_context', 'show_done', 'show_due_only',
];
const BOUNDED_RANGES = ['overdue', 'today', 'tomorrow', 'week', 'month'];

let onApplied = null;

/**
 * Run after a filter or tab change has swapped in the new list.
 * @param {function} callback
 */
export function setFilterAppliedCallback(callback) {
    onApplied = callback;
}

/**
 * Apply a filter by fetching partial content.
 * @param {Event|null} event - Click event (default is prevented)
 * @param {string} url - Filter URL
 */
export function applyFilter(event, url) {
    event?.preventDefault();

    // Update URL without reload
    window.history.pushState({}, '', url);

    // Optimistic update: reflect the new toggle/sort state immediately,
    // independent of network latency, so taps feel instant.
    updateFilterUI(url);

    // Fetch partial content
    const fetchUrl = new URL(url);
    fetchUrl.searchParams.set('partial', '1');

    const list = document.querySelector('.todo-list');
    if (list) list.setAttribute('aria-busy', 'true');

    fetch(fetchUrl)
        .then(response => response.text())
        .then(html => {
            if (list) {
                list.innerHTML = html;
                list.removeAttribute('aria-busy');
            }
            if (onApplied) onApplied();
        });
}

/**
 * Update tabs and the Add form to reflect the current state.
 * @param {string} currentUrl - Current URL
 */
export function updateFilterUI(currentUrl) {
    const url = new URL(currentUrl);
    const showDone = url.searchParams.get('show_done') === '1';
    const sortMode = url.searchParams.get('sort_mode') || currentSortMode() || 'topic';

    document.querySelectorAll('.filter-link[data-filter="sort"]').forEach(link => {
        const href = new URL(link.href);
        link.classList.toggle('active', href.searchParams.get('sort_mode') === sortMode);
        href.searchParams.set('show_done', showDone ? '1' : '0');
        link.href = href.toString();
    });

    const addForm = document.getElementById('addForm');
    if (addForm) {
        const addUrl = new URL(addForm.action);
        addUrl.searchParams.set('show_done', showDone ? '1' : '0');
        addUrl.searchParams.set('sort_mode', sortMode);
        addForm.action = addUrl.toString();
    }
}

function currentSortMode() {
    const active = document.querySelector('.view-tab.filter-link.active');
    return active ? new URL(active.href).searchParams.get('sort_mode') : null;
}

function updateBadge(form) {
    const data = new FormData(form);
    const count = Number((data.get('filter_due') || 'any') !== 'any')
        + Number(data.has('filter_project'))
        + Number(data.has('filter_context'));
    const menu = document.getElementById('filter-menu');
    const badge = menu?.querySelector('.filter-count');
    if (badge) {
        badge.textContent = String(count);
        badge.hidden = count === 0;
    }
    menu?.classList.toggle('has-active', count > 0 || data.has('show_done'));
}

function submitFilter(form) {
    const url = new URL(window.location.href);
    FILTER_PARAMS.forEach(key => url.searchParams.delete(key));

    const data = new FormData(form);
    url.searchParams.set('filter_set', '1');
    for (const [key, value] of data) {
        if (key !== 'filter_set' && key !== 'show_done') url.searchParams.append(key, value);
    }
    url.searchParams.set('show_done', data.has('show_done') ? '1' : '0');
    const sort = currentSortMode();
    if (sort) url.searchParams.set('sort_mode', sort);

    applyFilter(null, url.toString());
}

function syncUndated(form) {
    const due = form.querySelector('#filter-due');
    const undated = form.querySelector('#filter-undated');
    if (due && undated) undated.disabled = !BOUNDED_RANGES.includes(due.value);
}

function resetForm(form) {
    const due = form.querySelector('#filter-due');
    if (due) due.value = 'any';
    form.querySelectorAll('input[name="filter_project"], input[name="filter_context"], #filter-undated')
        .forEach(box => { box.checked = false; });
}

/**
 * Remove the filter a chip stands for.
 * @param {HTMLFormElement} form
 * @param {HTMLElement} chip - .filter-chip with data-remove (and data-value)
 */
function removeChip(form, chip) {
    const kind = chip.dataset.remove;
    if (kind === 'due') {
        const due = form.querySelector('#filter-due');
        if (due) due.value = 'any';
        const undated = form.querySelector('#filter-undated');
        if (undated) undated.checked = false;
        return;
    }
    const name = kind === 'project' ? 'filter_project' : 'filter_context';
    const value = (chip.dataset.value || '').toLowerCase();
    form.querySelectorAll(`input[name="${name}"]`).forEach(box => {
        if (box.value.toLowerCase() === value) box.checked = false;
    });
}

/**
 * Wire up the filter panel and the chips above the list. Call once.
 */
export function initFilterForm() {
    const form = document.getElementById('filter-form');
    if (!form) return;

    form.addEventListener('submit', e => e.preventDefault());
    form.addEventListener('change', () => {
        syncUndated(form);
        updateBadge(form);
        submitFilter(form);
    });

    // Reset buttons live in the panel and in the "nothing matches" message;
    // chips are re-rendered with each partial reload, hence delegation.
    document.addEventListener('click', e => {
        const reset = e.target.closest('[data-filter-reset]');
        const chip = e.target.closest('.filter-chip');
        if (!reset && !chip) return;
        e.preventDefault();
        if (reset) {
            resetForm(form);
        } else {
            removeChip(form, chip);
        }
        syncUndated(form);
        updateBadge(form);
        submitFilter(form);
    });
}
