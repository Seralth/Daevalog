// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// The history list shows 30 fights, 30 more on Show more; a group's
// heading folds it, and a folded group takes none of the fights shown.
Item {
    HistoryPages {
        id: pages
    }

    function fights(prefix, n) {
        const out = []
        for (let i = 0; i < n; ++i)
            out.push({ "id": prefix + i })
        return out
    }

    TestCase {
        name: "HistoryPages"

        function init() {
            pages.reset()
        }

        function test_thirty_then_thirty_more() {
            const groups = [{ "title": "", "fights": fights("f", 75) }]
            let view = pages.cut(groups)
            compare(view.groups[0].fights.length, 30)
            compare(view.groups[0].count, 75)
            compare(view.more, true)
            pages.more()
            compare(pages.cut(groups).groups[0].fights.length, 60)
            pages.more()
            view = pages.cut(groups)
            compare(view.groups[0].fights.length, 75)
            compare(view.more, false, "all shown")
        }

        function test_the_page_runs_across_groups() {
            const groups = [{ "title": "A", "fights": fights("a", 20) },
                            { "title": "B", "fights": fights("b", 20) },
                            { "title": "C", "fights": fights("c", 5) }]
            const view = pages.cut(groups)
            compare(view.groups.length, 2, "C waits for Show more")
            compare(view.groups[1].fights.length, 10)
            compare(view.more, true)
        }

        function test_a_folded_group_shows_its_heading_only() {
            const groups = [{ "title": "A", "fights": fights("a", 20) },
                            { "title": "B", "fights": fights("b", 20) }]
            pages.toggle("A")
            let view = pages.cut(groups)
            compare(view.groups[0].folded, true)
            compare(view.groups[0].fights.length, 0)
            compare(view.groups[0].count, 20)
            compare(view.groups[1].fights.length, 20, "B gets the page")
            compare(view.more, false)
            pages.toggle("A")
            view = pages.cut(groups)
            compare(view.groups[0].folded, false)
            compare(view.groups[0].fights.length, 20)
            compare(view.groups[1].fights.length, 10)
        }

        function test_the_list_layout_does_not_fold() {
            pages.toggle("")
            compare(pages.cut([{ "title": "", "fights": fights("f", 3) }]).groups[0].fights.length, 3)
        }

        function test_reset_opens_every_group_and_goes_back_to_thirty() {
            const groups = [{ "title": "A", "fights": fights("a", 50) }]
            pages.toggle("A")
            pages.more()
            pages.reset()
            const view = pages.cut(groups)
            compare(view.groups[0].folded, false)
            compare(view.groups[0].fights.length, 30)
        }
    }
}
