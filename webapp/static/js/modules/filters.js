/**
 * Filter and sort UI functionality.
 */

/**
 * Apply a filter by fetching partial content.
 * @param {Event} event - Click event
 * @param {string} url - Filter URL
 */
export function applyFilter(event, url) {
    event.preventDefault();

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
        });
}

/**
 * Swap the leading ☐/☑ checkbox glyph on a toggle link, keeping its label.
 * @param {HTMLElement} link - The toggle filter link
 * @param {boolean} checked - Whether the toggle is on
 */
function setToggleGlyph(link, checked) {
    const glyph = checked ? '☑' : '☐';
    // Strip any existing leading glyph + spacing, then re-prepend the right one.
    const label = link.textContent.replace(/^\s*[☐☑]\s*/, '').trim();
    link.textContent = `${glyph} ${label}`;
}

/**
 * Update filter UI to reflect current state.
 * @param {string} currentUrl - Current URL
 */
export function updateFilterUI(currentUrl) {
    const url = new URL(currentUrl);
    const showDone = url.searchParams.get('show_done') === '1';
    const showDueOnly = url.searchParams.get('show_due_only') === '1';
    const sortMode = url.searchParams.get('sort_mode') || 'topic';

    document.querySelectorAll('.filter-link').forEach(link => {
        const filter = link.dataset.filter;
        const newHref = new URL(link.href);

        if (filter === 'sort') {
            // Sort link: active when its sort_mode matches the current one.
            const linkSort = newHref.searchParams.get('sort_mode');
            link.classList.toggle('active', linkSort === sortMode);
            newHref.searchParams.set('show_done', showDone ? '1' : '0');
            newHref.searchParams.set('show_due_only', showDueOnly ? '1' : '0');
        } else if (filter === 'show_done') {
            // Toggle link: active (and ☑) when the filter is on; href toggles it.
            link.classList.toggle('active', showDone);
            setToggleGlyph(link, showDone);
            newHref.searchParams.set('show_done', showDone ? '0' : '1');
            newHref.searchParams.set('show_due_only', showDueOnly ? '1' : '0');
            newHref.searchParams.set('sort_mode', sortMode);
        } else if (filter === 'show_due_only') {
            link.classList.toggle('active', showDueOnly);
            setToggleGlyph(link, showDueOnly);
            newHref.searchParams.set('show_done', showDone ? '1' : '0');
            newHref.searchParams.set('show_due_only', showDueOnly ? '0' : '1');
            newHref.searchParams.set('sort_mode', sortMode);
        }

        link.href = newHref.toString();
    });

    // Update the Add form action to preserve filters
    const addForm = document.getElementById('addForm');
    if (addForm) {
        const addUrl = new URL(addForm.action);
        addUrl.searchParams.set('show_done', showDone ? '1' : '0');
        addUrl.searchParams.set('show_due_only', showDueOnly ? '1' : '0');
        addUrl.searchParams.set('sort_mode', sortMode);
        addForm.action = addUrl.toString();
    }
}
