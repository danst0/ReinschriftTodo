"""Pytest fixtures for Reinschrift webapp tests."""

import os
import sys
import tempfile
from typing import Generator

import pytest

# Add webapp directory to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app


@pytest.fixture
def app():
    """Create application for testing."""
    app = create_app('testing')
    app.config.update({
        'TESTING': True,
        'WTF_CSRF_ENABLED': False,
        'SECRET_KEY': 'test-secret-key',
    })
    yield app


@pytest.fixture
def client(app):
    """Create test client."""
    return app.test_client()


@pytest.fixture
def app_context(app):
    """Create application context."""
    with app.app_context():
        yield


@pytest.fixture
def temp_todo_file() -> Generator[str, None, None]:
    """Create a temporary todo file for testing."""
    content = """# Todos

- [ ] Buy groceries +shopping @errands due:2026-01-20T12:00 ^abc123
- [ ] Call doctor +health @phone due:2026-01-21T09:00 ^def456
- [x] Finish report +work @office due:2026-01-15T17:00 ✅ 2026-01-15 ^ghi789
- [ ] Review PR +dev @computer due:2026-01-22T10:00 rec:daily ^jkl012
- [ ] Plan vacation +personal due:9999-12-31T00:00 ^mno345
- [ ] Meeting notes +work @office ~note:"Discuss Q1 goals" ^pqr678

---

## Archive
"""
    with tempfile.NamedTemporaryFile(mode='w', suffix='.md', delete=False) as f:
        f.write(content)
        f.flush()
        yield f.name

    os.unlink(f.name)


@pytest.fixture
def sample_todo_line():
    """Sample todo line for parser testing."""
    return "- [ ] Buy groceries +shopping @errands due:2026-01-20T12:00 ^abc123"


@pytest.fixture
def sample_done_todo_line():
    """Sample completed todo line for parser testing."""
    return "- [x] Finish report +work @office due:2026-01-15T17:00 ✅ 2026-01-15 ^ghi789"


@pytest.fixture
def sample_recurring_todo_line():
    """Sample recurring todo line for parser testing."""
    return "- [ ] Review PR +dev @computer due:2026-01-22T10:00 rec:daily ^jkl012"


@pytest.fixture
def sample_sometime_todo_line():
    """Sample 'sometime' todo line for parser testing."""
    return "- [ ] Plan vacation +personal due:9999-12-31T00:00 ^mno345"


@pytest.fixture
def sample_note_todo_line():
    """Sample todo line with note for parser testing."""
    return '- [ ] Meeting notes +work @office ~note:"Discuss Q1 goals" ^pqr678'
