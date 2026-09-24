#!/usr/bin/env python3
"""make_fixtures.py DIR -- the documents the LibreOffice live tests open.

Written by LibreOffice itself, in the Microsoft formats a real user would
hand Syn: sales.xlsx (a `data` sheet of 24 rows, and a `Q3 sales` sheet
whose name has a space, because quoting one is where selectors break),
memo.docx (four paragraphs) and deck.pptx (empty). Regenerated on every
run, so a test never inherits the last run's edits.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lo_host import Office, prop  # noqa: E402
from com.sun.star.text.ControlCharacter import PARAGRAPH_BREAK  # noqa: E402

REGIONS = ["North", "South", "East", "West"]
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"]


def main(out):
    os.makedirs(out, exist_ok=True)
    office = Office("excel", visible=False)
    try:
        d = office.desktop
        hidden = (prop("Hidden", True),)

        wb = d.loadComponentFromURL("private:factory/scalc", "_blank", 0, hidden)
        ws = wb.Sheets.getByIndex(0)
        ws.Name = "data"
        rows = [["Region", "Month", "Units", "Price"]]
        for i, m in enumerate(MONTHS):
            for j, r in enumerate(REGIONS):
                rows.append([r, m, 40 + (i * 7 + j * 13) % 60, 12.5 + j])
        ws.getCellRangeByPosition(0, 0, 3, len(rows) - 1).setDataArray(tuple(tuple(r) for r in rows))
        wb.Sheets.insertNewByName("Q3 sales", 1)
        q3 = wb.Sheets.getByName("Q3 sales")
        q3.getCellRangeByName("A1:B3").setDataArray((("Site", "kWh"), ("Bearspaw", 3082637.6), ("Whitehorn", 2558802.0)))
        wb.storeToURL(office_url(out, "sales.xlsx"), (prop("FilterName", "Calc MS Excel 2007 XML"),))
        wb.close(True)

        doc = d.loadComponentFromURL("private:factory/swriter", "_blank", 0, hidden)
        # One paragraph each: a newline inside setString is a line break,
        # not a new paragraph, and the first run of this fixture came out as
        # a one-paragraph memo that made every p1 read fail.
        text = doc.Text
        cur = text.createTextCursor()
        for i, line in enumerate(["Site visit memo", "Prepared for the energy team.", "Findings follow.", "End of memo."]):
            if i:
                text.insertControlCharacter(cur, PARAGRAPH_BREAK, False)
            text.insertString(cur, line, False)
        doc.storeToURL(office_url(out, "memo.docx"), (prop("FilterName", "MS Word 2007 XML"),))
        doc.close(True)

        deck = d.loadComponentFromURL("private:factory/simpress", "_blank", 0, hidden)
        deck.storeToURL(office_url(out, "deck.pptx"), (prop("FilterName", "Impress MS PowerPoint 2007 XML"),))
        deck.close(True)
    finally:
        office.stop()
    print("fixtures in %s: sales.xlsx memo.docx deck.pptx" % out)


def office_url(out, name):
    import uno
    return uno.systemPathToFileUrl(os.path.abspath(os.path.join(out, name)))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "testbed/docs-lo")
