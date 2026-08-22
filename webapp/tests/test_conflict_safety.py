"""Tests for concurrent-write safety.

These cover the failure that let a completed todo reappear as open: a stale
whole-file write landing on top of a newer state, unnoticed.
"""

import os
import sys
import tempfile

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.exceptions import ConflictError
from app.routes.todo import _resolve_index
from app.services import storage
from app.services.parser import find_line_by_marker, marker_of
from app.services.undo_service import content_hash, pop_undo, push_undo, stamp_last_result


class TestMarkerResolution:
    """A line may carry several ``^id`` tokens — only the last one is ours."""

    def test_marker_is_the_trailing_id(self):
        line = '- [x] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ✅ 2025-12-24 ^gizxoy'
        assert marker_of(line) == 'gizxoy'

    def test_glued_id_is_only_a_fallback(self):
        line = '- [x] Meditation ✅ 2026-01-01^1d2hnl ^5szvah'
        assert marker_of(line) == '5szvah'

    def test_line_without_marker(self):
        assert marker_of('- [ ] Bettwäsche tauschen due:2026-01-10') is None

    def test_shared_block_link_does_not_capture_a_foreign_line(self):
        lines = [
            '- [ ] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ^aaa1',
            '- [ ] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ^bbb2',
        ]
        assert find_line_by_marker(lines, 'bbb2') == 1
        assert find_line_by_marker(lines, 'aaa1') == 0

    def test_falls_back_to_a_token_match(self):
        lines = ['- [ ] Nur ein Blocklink ^3pip9m ^aaa1']
        assert find_line_by_marker(lines, '3pip9m') == 0


class TestResolveIndex:
    """The index from a rendered page may be stale; the marker is not."""

    def test_marker_wins_over_a_shifted_index(self):
        lines = ['- [ ] Neu ^new1', '- [ ] Alt ^old1']
        # The page was rendered when "Alt" sat on line 0.
        assert _resolve_index(lines, 0, 'old1') == 1

    def test_index_is_used_when_no_marker_is_supplied(self):
        lines = ['- [ ] Neu ^new1', '- [ ] Alt ^old1']
        assert _resolve_index(lines, 0, None) == 0

    def test_unknown_marker_falls_back_to_the_index(self):
        lines = ['- [ ] Neu ^new1']
        assert _resolve_index(lines, 0, 'gone') == 0


@pytest.fixture
def local_file(app):
    """Point storage at a temp file and return its path."""
    with tempfile.NamedTemporaryFile(mode='w', suffix='.md', delete=False) as f:
        f.write('- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n')
        path = f.name
    app.config['USE_WEBDAV'] = False
    app.config['TODO_PATH'] = path
    yield path
    os.unlink(path)


class TestWriteGuard:
    """A write is checked against the read it was derived from."""

    def test_foreign_write_is_reported_not_overwritten(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()

            # Another client stores something new.
            foreign = content.replace('- [ ] Zwei ^bbb2', '- [x] Zwei ✅ 2026-08-22 ^bbb2')
            os.utime(local_file, (0, 0))  # make sure the mtime differs
            with open(local_file, 'w', encoding='utf-8') as f:
                f.write(foreign)

            with pytest.raises(ConflictError):
                storage.write_content(content.replace('- [ ] Eins', '- [x] Eins'))

        with open(local_file, encoding='utf-8') as f:
            assert f.read() == foreign, 'the foreign write must survive'

    def test_undisturbed_write_succeeds(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            storage.write_content(content.replace('- [ ] Eins', '- [x] Eins'))

        with open(local_file, encoding='utf-8') as f:
            assert f.read().startswith('- [x] Eins')

    def test_force_write_bypasses_the_guard(self, app, local_file):
        with app.test_request_context():
            storage.read_content()
            os.utime(local_file, (0, 0))
            storage.force_write_content('- [ ] Überschrieben ^ccc3\n')

        with open(local_file, encoding='utf-8') as f:
            assert f.read() == '- [ ] Überschrieben ^ccc3\n'

    def test_a_second_write_in_one_request_is_not_a_false_conflict(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            storage.write_content(content + '- [ ] Drei ^ccc3\n')
            # The recurrence spawn writes again right after the toggle.
            storage.write_content(content + '- [ ] Drei ^ccc3\n- [ ] Vier ^ddd4\n')

        with open(local_file, encoding='utf-8') as f:
            assert '^ddd4' in f.read()


class TestUndoGuard:
    """Undo restores a whole file — only over the state it was recorded for."""

    def test_stamp_records_the_produced_state(self, app):
        with app.test_request_context():
            push_undo('vorher', 'toggle')
            stamp_last_result('nachher')
            entry = pop_undo()
            assert entry['expected_hash'] == content_hash('nachher')

    def test_unstamped_entry_stays_unguarded(self, app):
        with app.test_request_context():
            push_undo('vorher', 'toggle')
            assert pop_undo()['expected_hash'] is None

    def test_write_stamps_the_entry(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            push_undo(content, 'toggle')
            produced = content.replace('- [ ] Eins', '- [x] Eins')
            storage.write_content(produced)
            assert pop_undo()['expected_hash'] == content_hash(produced)


class TestConflictHandler:
    """A rejected write must read as a conflict, not as a server fault."""

    def test_api_conflict_is_409_json(self, app):
        @app.route('/api/_conflict_probe')
        def probe():
            raise ConflictError('alt', 'neu')

        response = app.test_client().get('/api/_conflict_probe')
        assert response.status_code == 409
        payload = response.get_json()
        assert payload['conflict'] is True
        assert 'Reload' in payload['error']

    def test_page_conflict_is_409_html(self, app):
        @app.route('/_conflict_probe')
        def probe():
            raise ConflictError('alt', 'neu')

        response = app.test_client().get('/_conflict_probe')
        assert response.status_code == 409
        assert b'changed elsewhere' in response.data
