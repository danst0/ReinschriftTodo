/**
 * Main application entry point.
 *
 * This file imports all modules and exposes necessary functions globally
 * for use in HTML onclick attributes and template references.
 */

import { setCsrfToken, getCsrfToken, fetchWithCsrf } from './modules/api.js';
import { setDate, setNextWeekend, setSometime, focusRelative } from './modules/date-picker.js';
import {
    initModal, openEditModal, closeModal, showCommentField,
    submitWithComment, deleteTodoFromModal,
    setModalDate, setModalNextWeekend, setModalSometime
} from './modules/modal.js';
import {
    setReloadCallback as setTodoReloadCallback,
    toggleTodo, postponeTodo, postponeGroup
} from './modules/todo-actions.js';
import {
    configureAi, handleSmartAdd, runSmartAdd, improveTodoWithAi
} from './modules/ai.js';
import { initKeyboard, resetSelection } from './modules/keyboard.js';
import { initVoice } from './modules/voice.js';
import { initSwipeGestures } from './modules/swipe.js';
import { initPullToRefresh } from './modules/pull-refresh.js';
import { autoReload, startAutoReload, manualReload, setReloadCallback } from './modules/auto-reload.js';
import { applyFilter, updateFilterUI } from './modules/filters.js';
import { initDragDrop, setDragReloadCallback } from './modules/drag-drop.js';
import { showUndoToast, setUndoReloadCallback } from './modules/undo-toast.js';
import { startNotificationCheck } from './modules/notifications.js';
import { initTitleAutocomplete, invalidateTitleCache } from './modules/autocomplete.js';
import { openShareDialog, configureShare } from './modules/share.js';
import { addToMyDay, removeFromMyDay, setMydayReloadCallback } from './modules/myday.js';

// Configuration - will be set by template
let appConfig = {
    csrfToken: '',
    smartAddUrl: '/api/parse',
    aiTimeoutMs: 30000,
    autoAiOnAdd: false,
    skipDeleteConfirm: true,
    titleAutocomplete: true,
    language: 'en',
    translations: {}
};

// Last improved marker for highlighting
let lastImprovedMarker = null;

/**
 * Initialize the application.
 * @param {object} config - Configuration from template
 */
function initApp(config = {}) {
    Object.assign(appConfig, config);

    // Set up API
    setCsrfToken(appConfig.csrfToken);

    // Set up AI
    configureAi({
        parseUrl: appConfig.smartAddUrl,
        timeoutMs: appConfig.aiTimeoutMs
    });

    // Set up modal
    const modal = document.getElementById('editModal');
    if (modal) {
        initModal(modal, handleReload);
    }

    // Set up todo actions reload callback
    setTodoReloadCallback(handleReload);

    // Set up auto-reload callback
    setReloadCallback(handleReloadComplete);

    // Set up keyboard navigation
    initKeyboard({
        toggleAdd,
        toggleSearch,
        closeModal,
        closeSettings,
        closeCheatsheet,
        openCheatsheet
    });

    // Set up voice input
    initVoice({ language: appConfig.language });

    // Initialize swipe gestures
    initSwipeGestures();

    // Initialize pull-to-refresh
    initPullToRefresh(handleReload);

    // Initialize drag-and-drop between sections
    setDragReloadCallback(handleReload);
    initDragDrop();

    // Set up undo toast
    setUndoReloadCallback(handleReload);

    // Set up "My Day" actions
    setMydayReloadCallback(handleReload);

    // Start browser notifications for due reminders
    startNotificationCheck();

    // Start auto-reload (every 30 seconds)
    startAutoReload(30000, handleReloadComplete);

    // Set up add form
    setupAddForm();

    // Bind title autocomplete to Add and Edit inputs
    initTitleAutocomplete({
        inputSelectors: ['#add-input', '#edit-title'],
        enabled: appConfig.titleAutocomplete !== false
    });

    // Configure share dialog translations
    configureShare(appConfig.translations || {});

    // Set up scroll position persistence
    setupScrollPersistence();

    console.log('Reinschrift JS loaded');
}

/**
 * Handle reload and re-initialize gestures.
 */
async function handleReload() {
    await autoReload();
}

/**
 * Called after reload completes.
 */
function handleReloadComplete() {
    initSwipeGestures();
    initDragDrop();
    if (lastImprovedMarker) {
        highlightMarker(lastImprovedMarker);
    }
    updateBottomNavState();
}

/**
 * Highlight a todo item by marker.
 */
function highlightMarker(marker) {
    if (!marker) return;
    const el = document.querySelector(`.todo-item[data-marker="${marker}"]`);
    if (el) {
        el.classList.add('pulse');
        el.style.outline = '2px solid #6c5ce7';
        setTimeout(() => {
            el.classList.remove('pulse');
            el.style.outline = '';
        }, 1800);
        lastImprovedMarker = null;

        const rect = el.getBoundingClientRect();
        const inViewport = rect.top >= 0 && rect.bottom <= window.innerHeight;
        if (!inViewport) {
            showAddConfirmation(el.querySelector('.todo-title')?.textContent || 'Todo hinzugefügt');
        }
    }
}

function showAddConfirmation(text) {
    let toast = document.getElementById('add-confirmation-toast');
    if (!toast) {
        toast = document.createElement('div');
        toast.id = 'add-confirmation-toast';
        toast.className = 'add-toast';
        document.body.appendChild(toast);
    }
    toast.textContent = '✓ ' + text;
    toast.classList.add('visible');
    setTimeout(() => toast.classList.remove('visible'), 2000);
}

/**
 * Set up the add form submission.
 */
function setupAddForm() {
    const addForm = document.getElementById('addForm');
    if (!addForm) return;

    addForm.onsubmit = async function(e) {
        e.preventDefault();

        const input = document.getElementById('add-input');
        const original = input ? input.value.trim() : '';
        if (!original) return;

        try {
            const res = await fetchWithCsrf('/api/add', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ title: original })
            });

            if (!res.ok) {
                throw new Error(`HTTP ${res.status}`);
            }

            const data = await res.json();
            this.reset();
            invalidateTitleCache();

            await autoReload();
            highlightMarker(data.marker);
            if (input) input.focus();

            if (appConfig.autoAiOnAdd && data.marker) {
                lastImprovedMarker = data.marker;
                improveTodoWithAi(data.marker, original, (m) => {
                    autoReload().then(() => highlightMarker(m));
                });
            }
        } catch (err) {
            console.error('Add failed', err);
            alert('Add failed');
        }
    };
}

/**
 * Set up scroll position persistence.
 */
function setupScrollPersistence() {
    window.addEventListener('beforeunload', () => {
        sessionStorage.setItem('scrollPos', window.scrollY);
    });

    window.addEventListener('load', () => {
        const scrollPos = sessionStorage.getItem('scrollPos');
        if (scrollPos) {
            window.scrollTo(0, parseInt(scrollPos));
            sessionStorage.removeItem('scrollPos');
        }
    });
}

/**
 * Toggle search form visibility.
 */
function toggleSearch() {
    const form = document.getElementById('search-form');
    const btn = document.getElementById('toggle-search');
    const input = document.getElementById('search-input');
    const isVisible = form.classList.toggle('visible');
    btn.classList.toggle('active', isVisible || hasSearchQuery());
    if (isVisible) {
        input.focus();
        document.getElementById('addForm').classList.remove('visible');
        document.getElementById('toggle-add').classList.remove('active');
    }
    updateBottomNavState();
}

/**
 * Toggle add form visibility.
 */
function toggleAdd() {
    const form = document.getElementById('addForm');
    const btn = document.getElementById('toggle-add');
    const input = document.getElementById('add-input');
    const isVisible = form.classList.toggle('visible');
    btn.classList.toggle('active', isVisible);
    if (isVisible) {
        input.focus();
        document.getElementById('search-form').classList.remove('visible');
        if (!hasSearchQuery()) {
            document.getElementById('toggle-search').classList.remove('active');
        }
    }
    updateBottomNavState();
}

function hasSearchQuery() {
    const searchInput = document.getElementById('search-input');
    return searchInput && searchInput.value.trim() !== '';
}

/**
 * Open settings modal.
 */
function openSettings() {
    const modal = document.getElementById('settingsModal');
    if (modal) {
        modal.style.display = 'block';
    }
}

/**
 * Close settings modal.
 */
function closeSettings() {
    const modal = document.getElementById('settingsModal');
    if (modal) {
        modal.style.display = 'none';
    }
}

/**
 * Open cheatsheet modal.
 */
function openCheatsheet() {
    const modal = document.getElementById('cheatsheetModal');
    if (modal) {
        modal.style.display = 'block';
    }
}

/**
 * Close cheatsheet modal.
 */
function closeCheatsheet() {
    const modal = document.getElementById('cheatsheetModal');
    if (modal) {
        modal.style.display = 'none';
    }
}

/**
 * Update bottom navigation active states.
 */
function updateBottomNavState() {
    const addForm = document.getElementById('addForm');
    const searchForm = document.getElementById('search-form');

    const addBtn = document.getElementById('bottom-nav-add');
    const searchBtn = document.getElementById('bottom-nav-search');

    if (addBtn) {
        addBtn.classList.toggle('active', addForm?.classList.contains('visible'));
    }
    if (searchBtn) {
        searchBtn.classList.toggle('active', searchForm?.classList.contains('visible') || hasSearchQuery());
    }
}

// Expose functions globally for HTML onclick attributes
window.ReinschriftApp = {
    init: initApp,
    toggleTodo,
    postponeTodo,
    postponeGroup,
    openEditModal: (lineIndex) => openEditModal(lineIndex, appConfig.translations),
    closeModal,
    showCommentField,
    submitWithComment,
    deleteTodoFromModal: () => deleteTodoFromModal(
        appConfig.skipDeleteConfirm,
        appConfig.translations.deleteConfirm
    ),
    setModalDate,
    setModalNextWeekend,
    setModalSometime,
    handleSmartAdd: (btn) => handleSmartAdd(btn, {
        timeoutMessage: appConfig.translations.aiTimeoutNotice,
        errorMessage: appConfig.translations.aiParseFailed
    }),
    toggleSearch,
    toggleAdd,
    openSettings,
    closeSettings,
    openCheatsheet,
    closeCheatsheet,
    applyFilter,
    autoReload: handleReload,
    manualReload,
    showUndoToast
};

// Also expose as simple globals for backwards compatibility
window.toggleTodo = window.ReinschriftApp.toggleTodo;
window.postponeTodo = window.ReinschriftApp.postponeTodo;
window.postponeGroup = window.ReinschriftApp.postponeGroup;
window.openEditModal = window.ReinschriftApp.openEditModal;
window.closeModal = window.ReinschriftApp.closeModal;
window.showCommentField = window.ReinschriftApp.showCommentField;
window.submitWithComment = window.ReinschriftApp.submitWithComment;
window.deleteTodoFromModal = window.ReinschriftApp.deleteTodoFromModal;
window.setModalDate = window.ReinschriftApp.setModalDate;
window.setModalNextWeekend = window.ReinschriftApp.setModalNextWeekend;
window.setModalSometime = window.ReinschriftApp.setModalSometime;
window.handleSmartAdd = window.ReinschriftApp.handleSmartAdd;
window.toggleSearch = window.ReinschriftApp.toggleSearch;
window.toggleAdd = window.ReinschriftApp.toggleAdd;
window.openSettings = window.ReinschriftApp.openSettings;
window.closeSettings = window.ReinschriftApp.closeSettings;
window.closeCheatsheet = window.ReinschriftApp.closeCheatsheet;
window.applyFilter = window.ReinschriftApp.applyFilter;
window.autoReload = window.ReinschriftApp.autoReload;
window.manualReload = window.ReinschriftApp.manualReload;
window.openShareDialog = openShareDialog;
window.ReinschriftApp.openShareDialog = openShareDialog;
window.addToMyDay = addToMyDay;
window.removeFromMyDay = removeFromMyDay;
window.ReinschriftApp.addToMyDay = addToMyDay;
window.ReinschriftApp.removeFromMyDay = removeFromMyDay;
