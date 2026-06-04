"""Business logic services for the todo application."""

from app.services.parser import (
    parse_line,
    parse_due_token,
    parse_due_input,
    extract_title,
    find_line_by_marker,
    capture_token,
    capture_all_tokens,
)

from app.services.storage import (
    read_content,
    write_content,
    load_settings,
    save_settings,
)

from app.exceptions import StorageError

from app.services.date_service import (
    current_date,
    next_due_date,
    calculate_postpone_date,
    recent_window_bounds,
)

from app.services.sorting import (
    sort_key_topic,
    sort_key_location,
    sort_key_date,
    sort_todos,
)

from app.services.todo_service import (
    load_todos,
    toggle_todo,
    rewrite_line,
    add_todo,
    insert_line,
    delete_todo,
    update_todo_by_marker,
    handle_toggle_with_recurrence,
    postpone_todo,
    postpone_todos_batch,
    set_myday,
)

from app.services.ai_service import (
    parse_nlp,
    collect_recent_context,
    build_context_reminders,
)

__all__ = [
    # Parser
    'parse_line',
    'parse_due_token',
    'parse_due_input',
    'extract_title',
    'find_line_by_marker',
    'capture_token',
    'capture_all_tokens',
    # Storage
    'read_content',
    'write_content',
    'load_settings',
    'save_settings',
    'StorageError',
    # Date
    'current_date',
    'next_due_date',
    'calculate_postpone_date',
    'recent_window_bounds',
    # Sorting
    'sort_key_topic',
    'sort_key_location',
    'sort_key_date',
    'sort_todos',
    # Todo
    'load_todos',
    'toggle_todo',
    'rewrite_line',
    'add_todo',
    'insert_line',
    'delete_todo',
    'update_todo_by_marker',
    'handle_toggle_with_recurrence',
    'postpone_todo',
    'postpone_todos_batch',
    'set_myday',
    # AI
    'parse_nlp',
    'collect_recent_context',
    'build_context_reminders',
]
