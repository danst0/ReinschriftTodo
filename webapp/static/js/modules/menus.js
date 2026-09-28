/**
 * Small popup menus (<details class="menu">) and expandable todo rows.
 *
 * Tapping a todo's text opens its details (note, postpone targets, edit)
 * right below it. Only one row is open at a time, and the open row survives
 * the background partial reload, which replaces the whole list.
 */

// Marker (or line index for hand-written lines without one) of the open row.
let openKey = null;

function rowKey(row) {
    return row.dataset.marker || `line:${row.dataset.lineIndex}`;
}

function findRowByKey(key) {
    return Array.from(document.querySelectorAll('.todo-list .todo-item'))
        .find(row => rowKey(row) === key) || null;
}

function setRowOpen(row, open) {
    const content = row.querySelector('.content');
    const details = row.querySelector('.todo-details');
    if (!content || !details) return;
    details.hidden = !open;
    content.setAttribute('aria-expanded', open ? 'true' : 'false');
}

/**
 * Open or close a row's details; called from the row text's onclick.
 * @param {HTMLElement} contentEl - The row's .content element
 */
export function toggleTodoDetails(contentEl) {
    const row = contentEl.closest('.todo-item');
    if (!row) return;
    const key = rowKey(row);
    const wasOpen = openKey === key && !row.querySelector('.todo-details')?.hidden;

    if (openKey) {
        const previous = findRowByKey(openKey);
        if (previous) setRowOpen(previous, false);
    }
    openKey = wasOpen ? null : key;
    if (!wasOpen) setRowOpen(row, true);
}

/**
 * Re-open the row that was open before the list was re-rendered.
 */
export function restoreTodoDetails() {
    if (!openKey) return;
    const row = findRowByKey(openKey);
    if (row) {
        setRowOpen(row, true);
    } else {
        openKey = null;
    }
}

function closeMenus(except = null) {
    document.querySelectorAll('details.menu[open]').forEach(menu => {
        if (menu !== except) menu.removeAttribute('open');
    });
}

/**
 * Wire up menu closing and keyboard access for rows. Call once.
 */
export function initMenus() {
    // Capture phase: menu items' own handlers stop propagation.
    document.addEventListener('click', (e) => {
        if (e.target.closest('.menu-item')) {
            closeMenus();
        } else {
            closeMenus(e.target.closest('details.menu'));
        }
    }, true);

    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') closeMenus();
    });

    // Rows' text is a button for keyboard users. Handled on the list so the
    // global shortcuts (Enter = edit, Space = toggle) don't fire as well.
    const list = document.querySelector('.todo-list');
    if (list) {
        list.addEventListener('keydown', (e) => {
            const content = e.target.closest?.('.content[role="button"]');
            if (!content || (e.key !== 'Enter' && e.key !== ' ')) return;
            e.preventDefault();
            e.stopPropagation();
            toggleTodoDetails(content);
        });
    }
}
