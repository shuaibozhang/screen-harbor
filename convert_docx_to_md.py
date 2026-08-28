from pathlib import Path
from docx import Document
from docx.document import Document as _Document
from docx.table import Table
from docx.text.paragraph import Paragraph
from docx.oxml.table import CT_Tbl
from docx.oxml.text.paragraph import CT_P


ROOT = Path(__file__).resolve().parent
DELIVERABLES = ROOT / "deliverables"


def blocks(parent):
    if isinstance(parent, _Document):
        parent_elm = parent.element.body
    else:
        raise TypeError("document expected")
    for child in parent_elm.iterchildren():
        if isinstance(child, CT_P):
            yield Paragraph(child, parent)
        elif isinstance(child, CT_Tbl):
            yield Table(child, parent)


def clean(text):
    return " ".join(text.replace("\xa0", " ").split())


def escape_cell(text):
    return clean(text).replace("|", "\\|") or " "


def table_to_md(table):
    rows = [[escape_cell(cell.text) for cell in row.cells] for row in table.rows]
    if not rows:
        return []
    # Single-cell tinted tables in the source are callouts rather than tabular data.
    if len(rows[0]) == 1:
        return [f"> {rows[0][0]}", ""]
    width = max(len(row) for row in rows)
    rows = [row + [" "] * (width - len(row)) for row in rows]
    out = ["| " + " | ".join(rows[0]) + " |"]
    out.append("| " + " | ".join(["---"] * width) + " |")
    for row in rows[1:]:
        out.append("| " + " | ".join(row) + " |")
    out.append("")
    return out


def convert(source, destination):
    doc = Document(source)
    out = []
    for block in blocks(doc):
        if isinstance(block, Table):
            out.extend(table_to_md(block))
            continue
        text = clean(block.text)
        if not text:
            continue
        style = block.style.name if block.style else "Normal"
        if style == "Title":
            out.extend([f"# {text}", ""])
        elif style == "Subtitle":
            out.extend([f"_{text}_", ""])
        elif style.startswith("Heading "):
            level = int(style.split()[-1]) + 1
            out.extend([f"{'#' * level} {text}", ""])
        elif style.startswith("List Bullet"):
            out.append(f"- {text}")
        elif style.startswith("List Number"):
            out.append(f"1. {text}")
        else:
            if out and (out[-1].startswith("- ") or out[-1].startswith("1. ")):
                out.append("")
            out.extend([text, ""])
    while out and not out[-1]:
        out.pop()
    destination.write_text("\n".join(out) + "\n", encoding="utf-8")


if __name__ == "__main__":
    jobs = [
        (DELIVERABLES / "FrameFlow_PRD_v1.0.docx", DELIVERABLES / "FrameFlow_PRD_v1.0.md"),
        (DELIVERABLES / "FrameFlow_Interaction_Spec_v1.0.docx", DELIVERABLES / "FrameFlow_Interaction_Spec_v1.0.md"),
    ]
    for src, dst in jobs:
        convert(src, dst)
        print(dst)
