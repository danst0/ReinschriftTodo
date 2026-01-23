"""Sorting service for todo items."""

from datetime import datetime
from typing import Any

from app.models.todo import TodoItem


def sort_key_topic(todo: TodoItem | dict[str, Any]) -> tuple:
    """Sort key for topic (project) sorting.

    Order: Project (asc), Title (asc), Context (asc)
    Items with project come before items without.

    Args:
        todo: TodoItem or dict with todo data.

    Returns:
        Tuple for sorting.
    """
    if isinstance(todo, dict):
        projects = todo.get('projects', [])
        contexts = todo.get('contexts', [])
        title = todo.get('title', '')
    else:
        projects = todo.projects
        contexts = todo.contexts
        title = todo.title

    p = projects[0] if projects else None
    c = contexts[0] if contexts else None
    return (
        0 if p else 1, p.lower() if p else "",
        title.lower(),
        0 if c else 1, c.lower() if c else ""
    )


def sort_key_location(todo: TodoItem | dict[str, Any]) -> tuple:
    """Sort key for location (context) sorting.

    Order: Context (asc), Title (asc), Project (asc)
    Items with context come before items without.

    Args:
        todo: TodoItem or dict with todo data.

    Returns:
        Tuple for sorting.
    """
    if isinstance(todo, dict):
        projects = todo.get('projects', [])
        contexts = todo.get('contexts', [])
        title = todo.get('title', '')
    else:
        projects = todo.projects
        contexts = todo.contexts
        title = todo.title

    p = projects[0] if projects else None
    c = contexts[0] if contexts else None
    return (
        0 if c else 1, c.lower() if c else "",
        title.lower(),
        0 if p else 1, p.lower() if p else ""
    )


def sort_key_date(todo: TodoItem | dict[str, Any]) -> tuple:
    """Sort key for date sorting.

    Order: Due date (asc), then Project sort
    Items without due date come first.

    Args:
        todo: TodoItem or dict with todo data.

    Returns:
        Tuple for sorting.
    """
    if isinstance(todo, dict):
        d = todo.get('due')
    else:
        d = todo.due

    key_project = sort_key_topic(todo)

    if d is None:
        return (0, datetime.min, key_project)
    else:
        return (1, d, key_project)


def sort_todos(todos: list, sort_mode: str) -> list:
    """Sort a list of todos by the specified mode.

    Args:
        todos: List of TodoItem or dict objects.
        sort_mode: Sort mode ('topic', 'location', 'date').

    Returns:
        Sorted list.
    """
    if sort_mode == 'location':
        return sorted(todos, key=sort_key_location)
    elif sort_mode == 'date':
        return sorted(todos, key=sort_key_date)
    else:  # topic (default)
        return sorted(todos, key=sort_key_topic)
