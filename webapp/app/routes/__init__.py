"""Route blueprints for the todo application."""

from app.routes.main import main_bp
from app.routes.auth import auth_bp
from app.routes.todo import todo_bp
from app.routes.api import api_bp

__all__ = ['main_bp', 'auth_bp', 'todo_bp', 'api_bp']
