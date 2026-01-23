/**
 * Keyboard navigation and shortcuts.
 */

let selectedIndex = -1;
let onToggleAdd = null;
let onToggleSearch = null;
let onCloseModal = null;
let onCloseSettings = null;
let onCloseCheatsheet = null;
let onOpenCheatsheet = null;

/**
 * Initialize keyboard navigation.
 * @param {object} handlers - Event handlers
 */
export function initKeyboard(handlers) {
    onToggleAdd = handlers.toggleAdd;
    onToggleSearch = handlers.toggleSearch;
    onCloseModal = handlers.closeModal;
    onCloseSettings = handlers.closeSettings;
    onCloseCheatsheet = handlers.closeCheatsheet;
    onOpenCheatsheet = handlers.openCheatsheet;

    document.addEventListener('keydown', handleKeydown);
}

/**
 * Update visual selection of todo items.
 */
function updateSelection() {
    const items = document.querySelectorAll('.todo-item');
    items.forEach((item, index) => {
        if (index === selectedIndex) {
            item.style.border = '2px solid #3584e4';
            item.scrollIntoView({ behavior: 'smooth', block: 'center' });
        } else {
            item.style.border = '1px solid #eee';
        }
    });
}

/**
 * Get the currently selected todo's line index.
 * @returns {number|null}
 */
function getSelectedLineIndex() {
    const items = document.querySelectorAll('.todo-item');
    if (selectedIndex === -1 || selectedIndex >= items.length) return null;

    const onclick = items[selectedIndex].getAttribute('onclick');
    const match = onclick ? onclick.match(/openEditModal\((\d+)\)/) : null;
    return match ? parseInt(match[1]) : null;
}

function handleKeydown(e) {
    // Ignore if inside input or textarea
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') {
        if (e.key === 'Escape') {
            e.target.blur();
        }
        return;
    }

    const items = document.querySelectorAll('.todo-item');

    if (e.key === '?') {
        if (onOpenCheatsheet) onOpenCheatsheet();
    } else if (e.key === 'Escape') {
        if (onCloseModal) onCloseModal();
        if (onCloseSettings) onCloseSettings();
        if (onCloseCheatsheet) onCloseCheatsheet();
    } else if (!e.ctrlKey && !e.metaKey && e.key === 'n') {
        e.preventDefault();
        if (onToggleAdd) onToggleAdd();
    } else if (!e.ctrlKey && !e.metaKey && (e.key === 'f' || e.key === '/')) {
        e.preventDefault();
        if (onToggleSearch) onToggleSearch();
    } else if (e.key === 'j' || e.key === 'ArrowDown') {
        if (selectedIndex < items.length - 1) {
            selectedIndex++;
            updateSelection();
        }
    } else if (e.key === 'k' || e.key === 'ArrowUp') {
        if (selectedIndex > 0) {
            selectedIndex--;
            updateSelection();
        } else if (selectedIndex === -1) {
            selectedIndex = 0;
            updateSelection();
        }
    } else if (e.key === ' ' && selectedIndex !== -1) {
        e.preventDefault();
        const link = items[selectedIndex].querySelector('.checkbox');
        if (link) link.click();
    } else if (e.key === 'Enter' && selectedIndex !== -1) {
        items[selectedIndex].click();
    } else if (e.key === 't' && selectedIndex !== -1) {
        const idx = getSelectedLineIndex();
        if (idx !== null) {
            window.location.href = '/postpone/' + idx + '/today';
        }
    } else if (e.key === '+' && selectedIndex !== -1) {
        const idx = getSelectedLineIndex();
        if (idx !== null) {
            window.location.href = '/postpone/' + idx + '/tomorrow';
        }
    } else if (e.key === 'w' && selectedIndex !== -1) {
        const idx = getSelectedLineIndex();
        if (idx !== null) {
            window.location.href = '/postpone/' + idx + '/weekend';
        }
    } else if (e.key === 's' && selectedIndex !== -1) {
        const idx = getSelectedLineIndex();
        if (idx !== null) {
            window.location.href = '/postpone/' + idx + '/sometime';
        }
    }
}

/**
 * Reset selection index (after reload).
 */
export function resetSelection() {
    selectedIndex = -1;
}
