/**
 * AI parsing and smart add functionality.
 */

import { getCsrfToken } from './api.js';

let smartAddUrl = '/api/parse';
let smartAddTimeoutMs = 30000;
let smartAddController = null;

/**
 * Configure AI module.
 * @param {object} config - Configuration object
 * @param {string} config.parseUrl - AI parse endpoint URL
 * @param {number} config.timeoutMs - Request timeout in milliseconds
 */
export function configureAi(config) {
    if (config.parseUrl) smartAddUrl = config.parseUrl;
    if (config.timeoutMs) smartAddTimeoutMs = config.timeoutMs;
}

/**
 * Call AI parse endpoint.
 * @param {object} options - Options
 * @param {HTMLElement|null} options.btn - Button to show loading state
 * @param {string} options.text - Text to parse
 * @param {boolean} options.returnParsed - Return parsed object instead of formatted string
 * @returns {Promise<object|string|null>}
 */
export async function callAiParse({ btn = null, text, returnParsed = false }) {
    if (!text) return null;

    const controller = buildSmartAddController();
    const timeoutId = setTimeout(() => controller.abort(), smartAddTimeoutMs);
    setSmartButtonState(btn, true);

    try {
        const res = await fetch(smartAddUrl, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRFToken': getCsrfToken()
            },
            body: JSON.stringify({ text }),
            signal: controller.signal
        });

        if (!res.ok) {
            throw new Error(`HTTP ${res.status}`);
        }

        const data = await res.json();
        if (data && data.error) {
            throw new Error(data.error);
        }

        return returnParsed ? { parsed: data } : formatSmartResult(data, text);
    } catch (err) {
        if (err.name === 'AbortError') {
            throw new Error('timeout');
        }
        throw err;
    } finally {
        clearTimeout(timeoutId);
        if (smartAddController === controller) {
            smartAddController = null;
        }
        setSmartButtonState(btn, false);
    }
}

/**
 * Set loading state on smart button.
 * @param {HTMLElement|null} btn - Button element
 * @param {boolean} loading - Loading state
 */
function setSmartButtonState(btn, loading) {
    if (!btn) return;
    if (loading) {
        btn.classList.add('loading');
        btn.disabled = true;
    } else {
        btn.classList.remove('loading');
        btn.disabled = false;
    }
}

/**
 * Build or reset the AbortController.
 * @returns {AbortController}
 */
function buildSmartAddController() {
    if (smartAddController) {
        smartAddController.abort();
    }
    smartAddController = new AbortController();
    return smartAddController;
}

/**
 * Format AI-parsed data into a todo string.
 * @param {object|null} data - Parsed data
 * @param {string} fallbackText - Original text as fallback
 * @returns {string}
 */
export function formatSmartResult(data, fallbackText) {
    if (!data) return fallbackText;
    const base = (data.title || data.task || fallbackText || '').trim();
    if (!base) return fallbackText;

    let formatted = base;

    if (data.due && data.due !== 'null' && data.due !== 'None') {
        formatted += ` due:${data.due}`;
    }

    // Handle both single context and list of contexts
    if (data.contexts && Array.isArray(data.contexts) && data.contexts.length > 0) {
        formatted += ' ' + data.contexts.map(c => `@${String(c).replace(/^@/, '')}`).join(' ');
    } else if (data.context && data.context !== 'null' && data.context !== 'None') {
        formatted += ` @${String(data.context).replace(/^@/, '')}`;
    }

    // Handle both single project and list of projects
    if (data.projects && Array.isArray(data.projects) && data.projects.length > 0) {
        formatted += ' ' + data.projects.map(p => `+${String(p).replace(/^\+/, '')}`).join(' ');
    } else if (data.project && data.project !== 'null' && data.project !== 'None') {
        formatted += ` +${String(data.project).replace(/^\+/, '')}`;
    }

    return formatted;
}

/**
 * Run smart add on input text.
 * @param {object} options - Options
 * @param {HTMLElement|null} options.btn - Button element
 * @param {string|null} options.textOverride - Override text instead of input value
 * @param {boolean} options.returnParsed - Return parsed object
 * @returns {Promise<object|string|null>}
 */
export async function runSmartAdd({ btn = null, textOverride = null, returnParsed = false } = {}) {
    const input = document.getElementById('add-input');
    const text = textOverride ?? (input ? input.value : '');
    if (!text) return null;

    const result = await callAiParse({ btn, text, returnParsed });
    if (returnParsed) {
        const parsed = result ? result.parsed : null;
        return parsed ? { parsed, formatted: formatSmartResult(parsed, text) } : null;
    }
    return result;
}

/**
 * Handle smart add button click.
 * @param {HTMLElement} btn - Button element
 * @param {object} config - Configuration with error messages
 */
export async function handleSmartAdd(btn, config = {}) {
    try {
        const formatted = await runSmartAdd({ btn });
        if (!formatted) {
            return;
        }
        const input = document.getElementById('add-input');
        input.value = formatted;
        input.style.backgroundColor = '#6c5ce733';
        setTimeout(() => input.style.backgroundColor = '', 1000);
    } catch (err) {
        if (err && err.message === 'timeout') {
            alert(config.timeoutMessage || 'AI took too long');
        } else {
            console.error('AI Error:', err);
            alert(`${config.errorMessage || 'AI failed'} ${err && err.message ? err.message : ''}`.trim());
        }
    }
}

/**
 * Improve an existing todo with AI parsing.
 * @param {string} marker - Todo marker ID
 * @param {string} originalText - Original todo title
 * @param {function} onSuccess - Callback on success
 */
export async function improveTodoWithAi(marker, originalText, onSuccess) {
    try {
        const result = await runSmartAdd({ textOverride: originalText, returnParsed: true });
        const parsed = result && result.parsed ? result.parsed : null;
        if (!parsed) return;

        const payload = {
            marker,
            title: parsed.title || originalText,
            note: parsed.note,
            due: parsed.due,
            contexts: parsed.contexts || (parsed.context ? [parsed.context] : []),
            projects: parsed.projects || (parsed.project ? [parsed.project] : []),
        };

        const res = await fetch('/api/improve', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRFToken': getCsrfToken()
            },
            body: JSON.stringify(payload),
        });

        if (!res.ok) {
            const errTxt = await res.text();
            console.error('Improve failed', errTxt);
            return;
        }

        if (onSuccess) {
            onSuccess(marker);
        }
    } catch (err) {
        if (err && err.message === 'timeout') {
            console.warn('AI improve timed out');
        } else {
            console.error('AI improve failed', err);
        }
    }
}
