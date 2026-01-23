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

    // Fetch partial content
    const fetchUrl = new URL(url);
    fetchUrl.searchParams.set('partial', '1');

    fetch(fetchUrl)
        .then(response => response.text())
        .then(html => {
            document.querySelector('.todo-list').innerHTML = html;
            // Update active classes in filters
            updateFilterUI(url);
        });
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
        const linkUrl = new URL(link.href);
        const linkSort = linkUrl.searchParams.get('sort_mode');
        const linkShowDone = linkUrl.searchParams.get('show_done');
        const linkShowDueOnly = linkUrl.searchParams.get('show_due_only');

        link.classList.remove('active');

        // Check if it's a sort link
        if (linkSort && linkSort === sortMode) {
            link.classList.add('active');
        }
        // Check if it's a toggle link
        if (linkShowDone !== null && linkShowDone !== (showDone ? '1' : '0')) {
            // This is the toggle link for show_done
            if (showDone) link.classList.add('active');
        }
        if (linkShowDueOnly !== null && linkShowDueOnly !== (showDueOnly ? '1' : '0')) {
            // This is the toggle link for show_due_only
            if (showDueOnly) link.classList.add('active');
        }

        // Update href to reflect new state
        const newHref = new URL(link.href);
        if (linkSort) {
            newHref.searchParams.set('show_done', showDone ? '1' : '0');
            newHref.searchParams.set('show_due_only', showDueOnly ? '1' : '0');
        } else if (linkShowDone !== null) {
            newHref.searchParams.set('show_done', showDone ? '0' : '1');
            newHref.searchParams.set('show_due_only', showDueOnly ? '1' : '0');
            newHref.searchParams.set('sort_mode', sortMode);
        } else if (linkShowDueOnly !== null) {
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
