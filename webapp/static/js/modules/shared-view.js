(function () {
    const root = document.querySelector('.share-container');
    if (!root) return;

    const token = root.dataset.token;
    const errorEl = document.getElementById('share-error');
    const listEl = document.getElementById('share-todo-list');
    const form = document.getElementById('share-add-form');

    function showError(msg) {
        errorEl.textContent = msg;
        errorEl.style.display = 'block';
    }
    function clearError() {
        errorEl.style.display = 'none';
        errorEl.textContent = '';
    }

    async function toggleTodo(item) {
        const marker = item.dataset.marker;
        if (!marker || item.classList.contains('completing')) return;
        clearError();
        item.classList.add('completing');
        try {
            const resp = await fetch(`/s/${encodeURIComponent(token)}/toggle/${encodeURIComponent(marker)}`, {
                method: 'POST',
                headers: { 'Accept': 'application/json' },
            });
            if (!resp.ok) {
                item.classList.remove('completing');
                showError('Konnte nicht abhaken.');
                return;
            }
            setTimeout(() => item.remove(), 300);
        } catch (err) {
            item.classList.remove('completing');
            showError('Netzwerkfehler.');
        }
    }

    listEl.addEventListener('click', (ev) => {
        const checkbox = ev.target.closest('.checkbox');
        if (!checkbox) return;
        const item = checkbox.closest('.share-todo');
        if (item) toggleTodo(item);
    });

    form.addEventListener('submit', async (ev) => {
        ev.preventDefault();
        clearError();
        const input = form.querySelector('input[name="title"]');
        const title = (input.value || '').trim();
        if (!title) return;
        const btn = form.querySelector('button');
        btn.disabled = true;
        try {
            const resp = await fetch(`/s/${encodeURIComponent(token)}/add`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json', 'Accept': 'application/json' },
                body: JSON.stringify({ title }),
            });
            if (!resp.ok) {
                showError('Konnte nicht hinzufügen.');
                return;
            }
            input.value = '';
            // Reload to render the new item (server-rendered, keeps ordering consistent).
            window.location.reload();
        } catch (err) {
            showError('Netzwerkfehler.');
        } finally {
            btn.disabled = false;
        }
    });
})();
