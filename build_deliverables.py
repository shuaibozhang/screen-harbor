from pathlib import Path
from docx import Document
from docx.shared import Inches, Pt, RGBColor
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK, WD_LINE_SPACING
from docx.enum.table import WD_TABLE_ALIGNMENT, WD_CELL_VERTICAL_ALIGNMENT
from docx.enum.section import WD_SECTION
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.enum.style import WD_STYLE_TYPE

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "deliverables"
OUT.mkdir(exist_ok=True)

INK = "15212B"
MUTED = "60707D"
ACCENT = "5B5CE2"
ACCENT_DARK = "3F40B8"
TEAL = "1F9D8A"
PALE = "F2F3FF"
PALE_TEAL = "EBF8F5"
LINE = "DDE2E7"
LIGHT = "F6F8FA"
WHITE = "FFFFFF"
RED = "B54747"
GOLD = "9A6700"
FONT_CN = "Microsoft YaHei"
FONT_EN = "Aptos"


def set_cell_shading(cell, fill):
    tc_pr = cell._tc.get_or_add_tcPr()
    shd = tc_pr.find(qn("w:shd"))
    if shd is None:
        shd = OxmlElement("w:shd")
        tc_pr.append(shd)
    shd.set(qn("w:fill"), fill)


def set_cell_margins(cell, top=100, start=140, bottom=100, end=140):
    tc = cell._tc
    tcPr = tc.get_or_add_tcPr()
    tcMar = tcPr.first_child_found_in("w:tcMar")
    if tcMar is None:
        tcMar = OxmlElement("w:tcMar")
        tcPr.append(tcMar)
    for m, value in (("top", top), ("start", start), ("bottom", bottom), ("end", end)):
        node = tcMar.find(qn(f"w:{m}"))
        if node is None:
            node = OxmlElement(f"w:{m}")
            tcMar.append(node)
        node.set(qn("w:w"), str(value))
        node.set(qn("w:type"), "dxa")


def set_table_geometry(table, widths_dxa, indent_dxa=120):
    table.autofit = False
    tbl = table._tbl
    tblPr = tbl.tblPr
    tblW = tblPr.find(qn("w:tblW"))
    if tblW is None:
        tblW = OxmlElement("w:tblW")
        tblPr.append(tblW)
    tblW.set(qn("w:w"), str(sum(widths_dxa)))
    tblW.set(qn("w:type"), "dxa")
    tblInd = tblPr.find(qn("w:tblInd"))
    if tblInd is None:
        tblInd = OxmlElement("w:tblInd")
        tblPr.append(tblInd)
    tblInd.set(qn("w:w"), str(indent_dxa))
    tblInd.set(qn("w:type"), "dxa")
    grid = tbl.tblGrid
    for child in list(grid):
        grid.remove(child)
    for width in widths_dxa:
        col = OxmlElement("w:gridCol")
        col.set(qn("w:w"), str(width))
        grid.append(col)
    for row in table.rows:
        for idx, cell in enumerate(row.cells):
            tcPr = cell._tc.get_or_add_tcPr()
            tcW = tcPr.find(qn("w:tcW"))
            if tcW is None:
                tcW = OxmlElement("w:tcW")
                tcPr.append(tcW)
            tcW.set(qn("w:w"), str(widths_dxa[idx]))
            tcW.set(qn("w:type"), "dxa")
            set_cell_margins(cell)


def set_repeat_table_header(row):
    trPr = row._tr.get_or_add_trPr()
    tblHeader = OxmlElement("w:tblHeader")
    tblHeader.set(qn("w:val"), "true")
    trPr.append(tblHeader)


def set_run(run, size=None, bold=None, color=INK, italic=None, font=FONT_CN):
    run.font.name = font
    run._element.get_or_add_rPr().rFonts.set(qn("w:ascii"), font)
    run._element.get_or_add_rPr().rFonts.set(qn("w:hAnsi"), font)
    run._element.get_or_add_rPr().rFonts.set(qn("w:eastAsia"), font)
    if size is not None:
        run.font.size = Pt(size)
    if bold is not None:
        run.bold = bold
    if italic is not None:
        run.italic = italic
    if color:
        run.font.color.rgb = RGBColor.from_string(color)


def set_paragraph_font(p, size=10.5, color=INK, bold=False):
    for r in p.runs:
        set_run(r, size=size, color=color, bold=bold)


def border_bottom(paragraph, color=ACCENT, size=18, space=8):
    pPr = paragraph._p.get_or_add_pPr()
    pBdr = pPr.find(qn("w:pBdr"))
    if pBdr is None:
        pBdr = OxmlElement("w:pBdr")
        pPr.append(pBdr)
    bottom = OxmlElement("w:bottom")
    bottom.set(qn("w:val"), "single")
    bottom.set(qn("w:sz"), str(size))
    bottom.set(qn("w:space"), str(space))
    bottom.set(qn("w:color"), color)
    pBdr.append(bottom)


def add_field(paragraph, field):
    run = paragraph.add_run()
    fldChar = OxmlElement("w:fldChar")
    fldChar.set(qn("w:fldCharType"), "begin")
    instr = OxmlElement("w:instrText")
    instr.set(qn("xml:space"), "preserve")
    instr.text = field
    fldChar2 = OxmlElement("w:fldChar")
    fldChar2.set(qn("w:fldCharType"), "end")
    run._r.extend([fldChar, instr, fldChar2])
    set_run(run, size=8.5, color=MUTED)


def setup_document(title, subtitle, doc_type, version="V1.0"):
    doc = Document()
    sec = doc.sections[0]
    sec.page_width = Inches(8.5)
    sec.page_height = Inches(11)
    sec.top_margin = Inches(0.82)
    sec.bottom_margin = Inches(0.78)
    sec.left_margin = Inches(0.9)
    sec.right_margin = Inches(0.9)
    sec.header_distance = Inches(0.35)
    sec.footer_distance = Inches(0.35)

    styles = doc.styles
    normal = styles["Normal"]
    normal.font.name = FONT_CN
    normal._element.rPr.rFonts.set(qn("w:eastAsia"), FONT_CN)
    normal.font.size = Pt(10.5)
    normal.font.color.rgb = RGBColor.from_string(INK)
    normal.paragraph_format.space_after = Pt(6)
    normal.paragraph_format.line_spacing = 1.22

    for name, size, before, after, color in [
        ("Title", 25, 0, 6, INK),
        ("Subtitle", 12.5, 0, 18, MUTED),
        ("Heading 1", 16, 18, 8, ACCENT_DARK),
        ("Heading 2", 12.5, 13, 6, INK),
        ("Heading 3", 11, 9, 4, TEAL),
    ]:
        s = styles[name]
        s.font.name = FONT_CN
        s._element.rPr.rFonts.set(qn("w:eastAsia"), FONT_CN)
        s.font.size = Pt(size)
        s.font.bold = name != "Subtitle"
        s.font.color.rgb = RGBColor.from_string(color)
        s.paragraph_format.space_before = Pt(before)
        s.paragraph_format.space_after = Pt(after)
        s.paragraph_format.keep_with_next = True

    for list_name in ["List Bullet", "List Number"]:
        s = styles[list_name]
        s.font.name = FONT_CN
        s._element.rPr.rFonts.set(qn("w:eastAsia"), FONT_CN)
        s.font.size = Pt(10.5)
        s.paragraph_format.left_indent = Inches(0.38)
        s.paragraph_format.first_line_indent = Inches(-0.19)
        s.paragraph_format.space_after = Pt(4)
        s.paragraph_format.line_spacing = 1.22

    if "Callout" not in styles:
        callout = styles.add_style("Callout", WD_STYLE_TYPE.PARAGRAPH)
    else:
        callout = styles["Callout"]
    callout.font.name = FONT_CN
    callout._element.rPr.rFonts.set(qn("w:eastAsia"), FONT_CN)
    callout.font.size = Pt(11)
    callout.font.color.rgb = RGBColor.from_string(ACCENT_DARK)
    callout.paragraph_format.left_indent = Inches(0.15)
    callout.paragraph_format.right_indent = Inches(0.15)
    callout.paragraph_format.space_before = Pt(7)
    callout.paragraph_format.space_after = Pt(10)
    callout.paragraph_format.line_spacing = 1.25

    header = sec.header
    hp = header.paragraphs[0]
    hp.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    r = hp.add_run(f"FRAMEFLOW  ·  {doc_type}")
    set_run(r, size=8, bold=True, color=MUTED)
    footer = sec.footer
    fp = footer.paragraphs[0]
    fp.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    r = fp.add_run(f"{version}  ·  2026.08.27   |   ")
    set_run(r, size=8.5, color=MUTED)
    add_field(fp, "PAGE")

    p = doc.add_paragraph()
    p.paragraph_format.space_before = Pt(20)
    p.paragraph_format.space_after = Pt(8)
    r = p.add_run("FRAMEFLOW")
    set_run(r, size=10, bold=True, color=ACCENT)
    p = doc.add_paragraph(style="Title")
    p.add_run(title)
    set_paragraph_font(p, size=25, color=INK, bold=True)
    p = doc.add_paragraph(style="Subtitle")
    p.add_run(subtitle)
    set_paragraph_font(p, size=12.5, color=MUTED)

    meta = doc.add_table(rows=4, cols=2)
    meta.alignment = WD_TABLE_ALIGNMENT.LEFT
    rows = [("文档类型", doc_type), ("版本", version), ("面向对象", "产品 / 设计 / 研发 / 测试"), ("产品阶段", "概念定义 → MVP")]
    for i, (k, v) in enumerate(rows):
        meta.cell(i, 0).text = k
        meta.cell(i, 1).text = v
        set_cell_shading(meta.cell(i, 0), PALE)
        for p0 in meta.cell(i, 0).paragraphs:
            set_paragraph_font(p0, 9, ACCENT_DARK, True)
        for p0 in meta.cell(i, 1).paragraphs:
            set_paragraph_font(p0, 9.5, INK, False)
        for c in meta.rows[i].cells:
            c.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
    set_repeat_table_header(meta.rows[0])
    set_table_geometry(meta, [1800, 7560])
    p = doc.add_paragraph()
    p.paragraph_format.space_after = Pt(16)
    border_bottom(p, ACCENT, 20, 10)
    return doc


def add_heading(doc, text, level=1):
    return doc.add_paragraph(text, style=f"Heading {level}")


def add_para(doc, text, bold_lead=None, style=None):
    p = doc.add_paragraph(style=style)
    if bold_lead and text.startswith(bold_lead):
        r = p.add_run(bold_lead)
        set_run(r, bold=True)
        r = p.add_run(text[len(bold_lead):])
        set_run(r)
    else:
        r = p.add_run(text)
        set_run(r)
    return p


def add_bullets(doc, items):
    for item in items:
        p = doc.add_paragraph(style="List Bullet")
        r = p.add_run(item)
        set_run(r)


def add_numbers(doc, items):
    for item in items:
        p = doc.add_paragraph(style="List Number")
        r = p.add_run(item)
        set_run(r)


def add_callout(doc, label, text, tone="blue"):
    table = doc.add_table(rows=1, cols=1)
    fill = PALE if tone == "blue" else PALE_TEAL
    set_cell_shading(table.cell(0, 0), fill)
    p = table.cell(0, 0).paragraphs[0]
    r = p.add_run(label + "  ")
    set_run(r, size=10.5, bold=True, color=ACCENT_DARK if tone == "blue" else TEAL)
    r = p.add_run(text)
    set_run(r, size=10.5, color=INK)
    set_table_geometry(table, [9360])
    set_repeat_table_header(table.rows[0])
    doc.add_paragraph().paragraph_format.space_after = Pt(1)


def add_table(doc, headers, rows, widths, header_fill=PALE):
    table = doc.add_table(rows=1, cols=len(headers))
    table.alignment = WD_TABLE_ALIGNMENT.LEFT
    table.style = "Table Grid"
    for i, h in enumerate(headers):
        cell = table.cell(0, i)
        cell.text = h
        set_cell_shading(cell, header_fill)
        cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
        for p in cell.paragraphs:
            set_paragraph_font(p, 9, ACCENT_DARK, True)
    set_repeat_table_header(table.rows[0])
    for row in rows:
        cells = table.add_row().cells
        for i, value in enumerate(row):
            cells[i].text = str(value)
            cells[i].vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
            for p in cells[i].paragraphs:
                set_paragraph_font(p, 9, INK, False)
    set_table_geometry(table, widths)
    doc.add_paragraph().paragraph_format.space_after = Pt(2)
    return table


def page_break(doc):
    doc.add_page_break()


def build_prd():
    doc = setup_document(
        "FrameFlow 跨平台录屏工具 PRD",
        "在一次录制中，同时获得可快速预览的轻量版与可交付的高清版",
        "产品需求文档（PRD）",
    )
    add_callout(doc, "一句话定义", "FrameFlow 是面向知识工作者与内容创作者的 macOS / Windows 本地录屏工具：框选即录、状态不打扰，并自动产出高清母版与轻量预览版。")
    add_heading(doc, "0. 文档摘要", 1)
    add_table(doc, ["项目", "结论"], [
        ("核心用户", "产品/研发协作、教程创作者、客服/销售演示、异步沟通者"),
        ("核心场景", "快速框选区域 → 录制 → 双版本生成 → 预览/复制/导出"),
        ("关键差异", "高清与低清双产物；比例一键调整；本地优先；悬浮状态条"),
        ("MVP 成功标准", "首次成功录制 < 2 分钟；双版本成功率 ≥ 98%；录制崩溃率 < 0.5%"),
    ], [1800, 7560])

    add_heading(doc, "1. 背景与机会", 1)
    add_para(doc, "现有录屏工具往往在“立刻分享”和“保留高清资产”之间做取舍：高质量文件体积大、上传慢；低码率文件适合消息流与小窗预览，却不适合后续剪辑或高清展示。用户还需要在录前反复调整区域尺寸、录中确认状态、录后手工压缩，操作链条被割裂。")
    add_para(doc, "FrameFlow 将这条链路重新组织为一个动作：用户只决定录什么，系统负责稳定录制，并在结束后生成两个用途明确的版本。")
    add_heading(doc, "1.1 用户问题", 2)
    add_bullets(doc, [
        "框选范围缺少比例约束，录出来才发现不适合 16:9、1:1 或竖屏画布。",
        "录制中状态不明确：是否已开始、录了多久、麦克风是否有声、剩余空间是否足够。",
        "大文件无法在聊天、工单或资料库中流畅预览；手动压缩又增加等待与决策成本。",
        "macOS 与 Windows 的权限、音频来源和录制提示不同，体验容易割裂。",
        "设置项常以编码器术语呈现，普通用户难以理解质量、体积和兼容性的关系。",
    ])
    add_heading(doc, "1.2 产品机会", 2)
    add_callout(doc, "机会判断", "把“双产物”从高级设置升级为产品主叙事：轻量版负责流转与预览，高清版负责归档、剪辑与交付。", "green")

    add_heading(doc, "2. 产品目标与边界", 1)
    add_table(doc, ["类型", "目标 / 非目标"], [
        ("目标 G1", "从启动到开始录制，熟练用户 ≤ 8 秒；新用户无需阅读教程。"),
        ("目标 G2", "一次录制可靠生成两个版本，且明确展示用途、格式、尺寸和体积。"),
        ("目标 G3", "录制控制始终可见但不遮挡内容，提供键盘优先路径。"),
        ("目标 G4", "macOS 与 Windows 保持一致信息架构，同时尊重系统级权限与交互。"),
        ("非目标", "MVP 不做完整视频剪辑器、云端团队空间、直播推流、多轨后期工程。"),
    ], [1800, 7560])
    add_heading(doc, "2.1 产品原则", 2)
    add_bullets(doc, [
        "先行动，后精调：默认值足够好，录前只暴露必要决策。",
        "状态可感知，不制造噪音：用颜色、计时与音量状态表达确定性。",
        "以用途命名质量：显示“高清母版 / 轻量预览”，而不是只显示码率。",
        "可逆与可恢复：区域可重画、比例可切换、录制可暂停、异常可恢复临时文件。",
        "本地优先：默认不上传，清楚标示文件位置、剩余空间与自动清理策略。",
    ])

    page_break(doc)
    add_heading(doc, "3. 用户与核心任务", 1)
    add_table(doc, ["用户", "典型任务", "成功标准"], [
        ("产品/研发协作者", "录制 Bug、交互或技术演示并发到聊天/工单", "预览立即可看，高清可追溯"),
        ("教程创作者", "录制软件操作，后续进入剪辑或知识库", "比例准确、母版清晰"),
        ("客服/销售", "快速解释流程或回答客户问题", "小文件易发送、兼容性高"),
        ("异步管理者", "录制周报、评审反馈或方案讲解", "录制轻、状态明确、可复制链接/文件"),
    ], [1900, 3950, 3510])
    add_heading(doc, "3.1 Jobs to be Done", 2)
    add_bullets(doc, [
        "当我要说明屏幕上的问题或流程时，我希望几秒内选中准确区域并开始录制，以免打断思路。",
        "当录制结束时，我希望直接拿到可发送的小文件和可保存的高清文件，而不需要理解压缩参数。",
        "当录制进行中时，我希望始终知道是否有声音、录了多久以及如何暂停/结束，避免无效录制。",
    ])

    add_heading(doc, "4. 核心体验与主流程", 1)
    add_numbers(doc, [
        "启动 FrameFlow：主窗口只呈现“开始录屏”、最近录制与快速设置。",
        "点击“开始录屏”或使用全局快捷键，应用进入屏幕遮罩与区域绘制模式。",
        "按住拖拽绘制录制区域；松开后显示尺寸、比例辅助线和位于区域上方的悬浮菜单。",
        "使用比例菜单切换自由、16:9、4:3、1:1、9:16，或微调边缘与位置。",
        "确认音频/摄像头状态，点击“录制”；3 秒倒计时后悬浮条切换为录制状态。",
        "录制中可暂停、继续、标记片段、开关麦克风、停止或取消。",
        "停止后立即进入结果页：先播放已就绪的轻量预览版，后台完成高清版本封装。",
        "用户复制轻量版、打开高清版、重命名、显示文件夹或开启一次性导出设置。",
    ])
    add_heading(doc, "4.1 关键状态机", 2)
    add_table(doc, ["状态", "进入条件", "主要界面", "可执行动作"], [
        ("Idle", "应用已启动", "主窗口/托盘", "开始录制、打开最近文件、设置"),
        ("Selecting", "点击开始录制", "全屏遮罩+十字光标", "绘制、取消、选择窗口/全屏"),
        ("Ready", "完成框选", "选区+悬浮菜单", "调比例、移动、选择输入、录制"),
        ("Countdown", "确认录制", "3-2-1 轻提示", "取消"),
        ("Recording", "倒计时完成", "录制状态条", "暂停、静音、标记、停止"),
        ("Paused", "点击暂停", "琥珀色状态条", "继续、停止、取消"),
        ("Processing", "停止录制", "结果页+双任务进度", "先预览低清、后台等待高清"),
        ("Done", "双版本完成", "结果页", "播放、复制、导出、打开目录、删除"),
        ("Recovering", "崩溃/断电后重启", "恢复卡片", "恢复、查看临时文件、丢弃"),
    ], [1250, 2200, 2600, 3310])

    add_heading(doc, "5. 功能需求", 1)
    add_heading(doc, "5.1 区域选择与比例", 2)
    add_table(doc, ["ID", "需求", "优先级", "验收摘要"], [
        ("REC-01", "拖拽绘制任意区域，实时显示宽×高与比例", "P0", "跨显示器可绘制；最小 160×90"),
        ("REC-02", "支持自由、16:9、4:3、1:1、9:16 与自定义比例", "P0", "切换后以中心为锚，自动贴合屏幕边界"),
        ("REC-03", "支持拖拽 8 个控制点与移动整个区域", "P0", "边缘吸附；Shift 临时锁比例"),
        ("REC-04", "窗口识别与一键选择全屏/当前窗口", "P1", "悬停高亮可录窗口"),
        ("REC-05", "记住最近比例与区域（按显示器）", "P1", "可在设置中关闭"),
    ], [950, 5200, 1000, 2210])
    add_heading(doc, "5.2 悬浮状态与控制", 2)
    add_table(doc, ["ID", "需求", "优先级", "验收摘要"], [
        ("CTL-01", "悬浮条默认位于选区上方，空间不足时自动翻转到内部或下方", "P0", "不超屏；可拖动"),
        ("CTL-02", "显示状态点、计时、麦克风电平、暂停/继续、停止", "P0", "关键状态无需展开菜单"),
        ("CTL-03", "录制时边框为克制红色，暂停变为琥珀色", "P0", "系统权限提示不可被遮挡"),
        ("CTL-04", "紧凑模式只显示状态、计时和停止", "P1", "支持自动收起与鼠标悬停展开"),
        ("CTL-05", "支持录制标记，结果页展示时间点", "P2", "快捷键可添加标记"),
    ], [950, 5200, 1000, 2210])

    add_heading(doc, "5.3 双版本生成", 2)
    add_callout(doc, "推荐实现", "录制阶段生成高质量中间流；停止后优先产出轻量预览，同时封装/转码高清母版。性能足够时，可选并行双编码，但必须以录制稳定性优先。")
    add_table(doc, ["能力", "高清母版（Master）", "轻量预览（Preview）"], [
        ("默认用途", "归档、剪辑、高清展示", "聊天、工单、小窗秒开"),
        ("默认格式", "MP4 / H.264", "WebM / VP9；兼容模式为 MP4 / H.264"),
        ("默认尺寸", "原始选区，最高 4K", "长边 960px，保持比例"),
        ("默认帧率", "30 fps，可设 60 fps", "15 fps，可设 24/30 fps"),
        ("质量策略", "CRF 18–22 或质量预设", "目标码率/体积优先，默认 0.6–1.2 Mbps"),
        ("音频", "AAC 128 kbps / Opus 可选", "Opus 48–64 kbps / AAC 64 kbps"),
        ("完成优先级", "第二完成", "第一完成，可立即播放/复制"),
    ], [1700, 3830, 3830])
    add_para(doc, "兼容性说明：WebM 适合网页与体积敏感场景，但部分原生播放器、演示软件或企业环境兼容性不如 MP4。产品应将“最佳压缩 / 最佳兼容”作为一级选择，把具体编码器放到高级设置。")

    page_break(doc)
    add_heading(doc, "5.4 导出与设置", 2)
    add_table(doc, ["设置组", "一级选项", "高级选项"], [
        ("高清版本", "格式、分辨率、帧率、质量", "编码器、CRF/码率、关键帧、硬件加速"),
        ("轻量版本", "格式、尺寸、体积偏好", "帧率、音频码率、关键帧、是否静音"),
        ("命名与路径", "保存位置、命名模板", "日期/应用名/显示器/序号变量"),
        ("录制输入", "麦克风、系统音频、摄像头", "降噪、声道、摄像头形状与位置"),
        ("行为", "倒计时、光标、点击效果、通知", "记住选区、自动复制、完成后动作"),
        ("存储", "保留临时文件、自动清理", "空间阈值、失败恢复、缓存上限"),
    ], [1800, 3500, 4060])
    add_heading(doc, "5.5 结果页与文件管理", 2)
    add_bullets(doc, [
        "单一播放器默认加载轻量版，并提供“切换高清”按钮；高清未完成时显示后台进度。",
        "两个产物作为同一次录制的一个“录制组”呈现，避免最近列表重复出现两条。",
        "明确显示格式、分辨率、时长、体积和保存位置；提供复制文件、拖拽分享、打开文件夹。",
        "删除录制组时默认同时删除两个版本；展开后可仅删除其中一个，并给出可恢复提示。",
        "失败时保留已成功版本，不因高清转码失败而阻塞轻量版使用。",
    ])

    add_heading(doc, "6. 权限与跨平台策略", 1)
    add_table(doc, ["主题", "macOS", "Windows", "产品策略"], [
        ("屏幕录制", "需系统隐私权限，授权后可能要求重启", "通常无需单独授权", "首次启动先做权限检查，提供系统设置直达"),
        ("系统音频", "依系统版本/捕获 API，需清楚说明能力", "WASAPI loopback", "自动检测；不可用时不显示无效选项"),
        ("麦克风", "需麦克风权限", "需隐私设置允许", "录前电平测试与权限修复入口"),
        ("系统托盘", "菜单栏常驻", "任务栏托盘常驻", "名称一致、系统习惯不同"),
        ("快捷键", "⌘/⌥ 语义", "Ctrl/Alt 语义", "界面按平台显示实际组合"),
    ], [1450, 2500, 2500, 2910])
    add_heading(doc, "6.1 首次使用", 2)
    add_numbers(doc, [
        "欢迎页解释“本地录制、双版本生成”并只保留一个主按钮。",
        "按需请求屏幕录制与麦克风权限，不在首屏一次性索取所有权限。",
        "完成 10 秒演示录制，结果页直接展示轻量与高清的体积差异。",
        "允许跳过演示；主界面持续显示未完成权限但不阻塞无需该权限的功能。",
    ])

    add_heading(doc, "7. 非功能需求", 1)
    add_table(doc, ["维度", "MVP 指标"], [
        ("启动", "冷启动 ≤ 2.5 秒；热启动 ≤ 1 秒（目标设备 P75）"),
        ("录制稳定性", "连续录制 2 小时不丢帧崩溃；崩溃率 < 0.5%"),
        ("资源", "1080p30 录制平均 CPU < 20%；内存 < 500MB（硬件编码开启）"),
        ("音画同步", "60 分钟录制末端偏差 ≤ 100ms"),
        ("恢复", "异常退出后可恢复最近一次临时录制，成功率 ≥ 95%"),
        ("隐私", "默认本地处理；不采集画面/音频内容；遥测可关闭"),
        ("可访问性", "键盘可完成主流程；控件有可读名称；状态不只依赖颜色"),
    ], [2100, 7260])

    add_heading(doc, "8. 数据与成功指标", 1)
    add_table(doc, ["层级", "指标", "目标"], [
        ("北极星", "每周成功完成并被打开/复制的录制组数", "发布后 8 周持续增长"),
        ("激活", "安装后 24 小时内成功完成首条录制", "≥ 65%"),
        ("效率", "从开始框选到倒计时开始的中位时长", "≤ 8 秒"),
        ("可靠性", "双版本均成功生成的录制占比", "≥ 98%"),
        ("价值", "轻量版被复制/拖出的录制占比", "≥ 35%"),
        ("留存", "完成首录用户的第 4 周留存", "≥ 25%"),
    ], [1500, 5600, 2260])
    add_para(doc, "事件采集只记录交互与技术元数据，例如录制时长区间、输出格式、成功/失败码、处理耗时；不采集文件名、窗口标题、画面内容或音频内容。")

    add_heading(doc, "9. MVP 范围与路线图", 1)
    add_table(doc, ["阶段", "范围"], [
        ("MVP / P0", "区域/全屏录制、比例预设、悬浮状态条、麦克风与系统音频、双版本、结果页、基础设置、失败恢复、macOS/Windows"),
        ("V1.1 / P1", "窗口选择、摄像头浮窗、鼠标点击效果、录制历史搜索、自定义比例、自动复制、压缩预估"),
        ("V1.2 / P2", "时间标记、静音片段、简单裁剪、团队预设、云盘导出、命令行与 API"),
        ("远期探索", "自动章节、语音转写、敏感信息遮挡、AI 摘要、团队评审链接"),
    ], [1600, 7760])

    add_heading(doc, "10. 风险、依赖与决策", 1)
    add_table(doc, ["风险 / 决策", "处理方式"], [
        ("并行双编码导致掉帧", "默认单高质量采集 + 录后转码；性能测试通过后再开放并行模式。"),
        ("WebM 兼容性", "默认根据场景推荐；始终提供 MP4 轻量版；结果页清楚标识兼容性。"),
        ("磁盘空间翻倍", "录前估算、低空间预警、缓存上限、失败自动清理与单版本模式。"),
        ("macOS 权限流复杂", "权限诊断页、系统设置直达、授权后状态轮询、清晰重启指引。"),
        ("区域菜单遮挡", "自动翻转、紧凑态、可拖动、录制开始后自动收起。"),
        ("编码器与容器组合不合法", "设置采用约束式选项，选择格式后只展示兼容编码器。"),
    ], [2800, 6560])
    add_heading(doc, "10.1 待确认产品决策", 2)
    add_bullets(doc, [
        "品牌名 FrameFlow 为工作名，正式命名前需完成商标与域名检查。",
        "MVP 的轻量版默认是 WebM/VP9 还是兼容优先的 MP4/H.264，需要用目标用户环境验证。",
        "高清生成是录制时并行编码，还是停止后转码；建议按硬件分级并以稳定性为准。",
        "是否默认录制系统音频；涉及隐私与平台能力，建议默认关闭并记住用户选择。",
    ])

    add_heading(doc, "11. MVP 验收清单", 1)
    add_bullets(doc, [
        "用户可在双屏、不同缩放比例环境下准确框选区域，悬浮菜单始终可见且不超出屏幕。",
        "比例切换、控制点拖拽和移动区域均无明显跳变，边界与最小尺寸规则一致。",
        "录制状态、暂停状态、麦克风静音和处理状态均有文本/图标/颜色三重反馈。",
        "停止后 3 秒内进入结果页；轻量版可先于高清版播放；任何一个版本失败不影响另一个。",
        "格式、尺寸、帧率与质量设置可保存为默认，并能恢复推荐设置。",
        "空间不足、权限缺失、编码失败、显示器断开与系统休眠均有可执行的恢复路径。",
        "同一套功能通过 macOS 与 Windows 的键盘、屏幕阅读器和 125%–200% 缩放测试。",
    ])
    path = OUT / "FrameFlow_PRD_v1.0.docx"
    doc.save(path)
    return path


def build_interaction():
    doc = setup_document(
        "FrameFlow 交互与界面规范",
        "从框选、录制到双版本交付的完整交互蓝图",
        "交互设计说明",
    )
    add_callout(doc, "体验目标", "用户在任何时刻都能回答三个问题：我正在录什么？录制是否正常？结束后会得到什么？")
    add_heading(doc, "0. 设计基线", 1)
    add_table(doc, ["维度", "规范"], [
        ("平台", "macOS 13+、Windows 10/11；共享信息架构，使用平台原生快捷键与权限语言"),
        ("视觉", "深色悬浮控件 + 中性主窗口；重点动作使用紫蓝色，录制状态使用克制红色"),
        ("网格", "8pt 基础网格；控件高度 32/36/44；圆角 8/12/16；最小点击目标 36×36"),
        ("动效", "120–180ms 状态过渡；只用于因果反馈，不使用持续炫技动画"),
        ("文本", "动词优先、结果明确：开始录制 / 暂停 / 完成并保存 / 复制轻量版"),
    ], [1900, 7460])

    add_heading(doc, "1. 信息架构", 1)
    add_table(doc, ["层级", "入口与内容"], [
        ("主窗口", "开始录屏、最近录制、快速输入状态、存储概览"),
        ("录制覆盖层", "模式切换、区域绘制、尺寸/比例、悬浮菜单"),
        ("录制状态条", "状态、计时、音频、暂停、停止、更多"),
        ("结果页", "播放器、双版本卡片、导出/复制、文件信息"),
        ("设置", "录制、高清输出、轻量输出、快捷键、存储、隐私与更新"),
        ("菜单栏/托盘", "开始录制、暂停/停止、最近录制、打开主窗口、退出"),
    ], [1900, 7460])
    add_heading(doc, "1.1 导航原则", 2)
    add_bullets(doc, [
        "主流程没有侧边栏跳转：开始录屏后进入独立覆盖层，完成后回到结果页。",
        "设置是独立窗口；从悬浮菜单打开时默认定位到相关分组。",
        "同一次录制的高清与轻量文件始终绑定为一个录制组，避免认知重复。",
        "危险动作放入更多菜单，并区分“取消录制”和“完成并保存”。",
    ])

    add_heading(doc, "2. 全局流程图", 1)
    add_table(doc, ["步骤", "用户意图", "系统响应", "退出/异常路径"], [
        ("1 启动", "准备录制", "检查权限与上次输入；主按钮聚焦", "权限缺失显示修复提示"),
        ("2 选择", "确定录什么", "全屏遮罩；拖拽实时显示尺寸", "Esc 返回；显示器断开重置"),
        ("3 调整", "适配比例/画布", "悬浮菜单出现；吸附与边界约束", "点击遮罩外重画"),
        ("4 录制", "开始讲解", "倒计时后计时；显示音量与状态", "取消需二次确认"),
        ("5 处理", "尽快拿到结果", "先生成轻量版；高清后台处理", "失败保留成功版本"),
        ("6 交付", "播放/分享/保存", "一组两版本；推荐动作突出", "可重新导出单个版本"),
    ], [1100, 2100, 3480, 2680])

    page_break(doc)
    add_heading(doc, "3. 主窗口", 1)
    add_heading(doc, "3.1 布局", 2)
    add_table(doc, ["区域", "内容与行为"], [
        ("顶部栏", "品牌、设置、帮助；Windows 包含窗口控制，macOS 遵循原生标题栏"),
        ("主行动区", "“开始录屏”主按钮；右侧下拉可直接选择区域/窗口/全屏"),
        ("输入状态", "麦克风、系统音频、摄像头；点击可切换来源，异常显示修复入口"),
        ("最近录制", "按录制组展示缩略图、名称、时长、时间、两种版本状态与快捷动作"),
        ("底部", "当前保存路径、剩余空间、版本号；低空间时升级为警告条"),
    ], [1800, 7560])
    add_heading(doc, "3.2 默认态与空状态", 2)
    add_bullets(doc, [
        "空状态标题：“录下重点，交付两个版本。”副文案解释轻量预览与高清母版。",
        "主按钮始终是视觉第一焦点；首次使用下方出现快捷键提示，不出现多页引导。",
        "最近录制最多首屏显示 5 条；滚动进入历史页，右键菜单提供重命名、显示文件、删除。",
        "后台生成高清时，该录制卡片显示细进度条，不阻塞再次开始录制。",
    ])

    add_heading(doc, "4. 区域选择覆盖层", 1)
    add_heading(doc, "4.1 视觉层级", 2)
    add_table(doc, ["元素", "规范"], [
        ("遮罩", "选区外黑色 52% 透明；选区内保持原屏幕亮度"),
        ("选区边框", "1.5px 紫蓝色；8 个 8px 控制点；高 DPI 按物理像素对齐"),
        ("十字光标", "绘制前显示水平/垂直辅助线；交点附近显示实时坐标或尺寸"),
        ("尺寸标签", "选区左下或内部安全位置显示“1920 × 1080 · 16:9”"),
        ("悬浮菜单", "优先居中位于选区上方 12px；空间不足自动下翻，永不跨屏"),
    ], [1900, 7460])
    add_heading(doc, "4.2 绘制规则", 2)
    add_numbers(doc, [
        "按下鼠标时记录起点；拖拽超过 4px 后进入绘制态，否则视为点击并尝试选择窗口。",
        "拖拽中实时更新边框、尺寸与当前比例；按住 Shift 锁定最近使用比例。",
        "松开后进入 Ready，焦点落在“开始录制”，同时显示 8 个控制点。",
        "点击选区外空白处开始重画；拖动选区内部移动；方向键每次移动 1px，Shift+方向键移动 10px。",
        "最小录制区域 160×90；超出屏幕时自动钳制，绝不静默改变目标显示器。",
    ])
    add_heading(doc, "4.3 比例菜单", 2)
    add_table(doc, ["选项", "行为", "显示文案"], [
        ("自由", "保持当前尺寸，控制点可独立调整", "自由"),
        ("16:9 / 4:3 / 1:1 / 9:16", "以当前中心为锚调整到最近可容纳尺寸", "16:9  横屏"),
        ("自定义", "输入宽高比或具体像素；校验最小值与屏幕边界", "自定义…"),
        ("常用尺寸", "1280×720、1920×1080 等；超过屏幕时等比缩小", "1920 × 1080"),
    ], [1800, 4300, 3260])
    add_callout(doc, "微交互", "切换比例时用 160ms ease-out 平滑调整边框；尺寸标签同步变化。若被屏幕边界限制，显示一次性提示“已按屏幕范围缩放”。", "green")

    add_heading(doc, "5. 选区上方悬浮菜单", 1)
    add_table(doc, ["顺序", "控件", "默认状态 / 行为"], [
        ("1", "录制模式", "区域；下拉可切换窗口、全屏"),
        ("2", "比例", "自由；显示当前比例，展开预设"),
        ("3", "尺寸", "只读尺寸；点击可输入精确宽高"),
        ("4", "输入", "麦克风、系统音频、摄像头图标；有声时电平反馈"),
        ("5", "开始录制", "主按钮；快捷键 Enter / ⌘↵ / Ctrl+Enter"),
        ("6", "更多", "倒计时、光标、设置、取消"),
    ], [800, 1700, 6860])
    add_para(doc, "菜单宽度不足时按优先级收敛：隐藏“尺寸输入”文字 → 输入控件合并为一个按钮 → 模式与比例合并到更多菜单。开始录制始终可见。")

    page_break(doc)
    add_heading(doc, "6. 倒计时与录制状态条", 1)
    add_heading(doc, "6.1 倒计时", 2)
    add_bullets(doc, [
        "默认 3 秒；数字显示在选区中心，背景为轻度磨砂圆形，避免全屏闪烁。",
        "每秒短促声音默认关闭；开启后尊重系统音量。Esc 可取消并回到 Ready。",
        "倒计时期间悬浮菜单缩为“正在开始 · 取消”，最后一拍后无跳变切换到录制条。",
    ])
    add_heading(doc, "6.2 录制条布局", 2)
    add_table(doc, ["位置", "元素", "交互"], [
        ("左", "红色状态点 + REC / 已暂停", "状态文本不只依赖颜色"),
        ("中左", "00:03:24", "录制时长；超过 1 小时显示 hh:mm:ss"),
        ("中", "麦克风电平 / 静音", "点击静音；长按/下拉切换设备"),
        ("中右", "暂停/继续", "空格或全局快捷键；暂停后条变琥珀色"),
        ("右", "完成", "方形停止图标 + 文案；立即停止并保存"),
        ("更多", "标记、摄像头、隐藏边框、取消录制", "取消需确认，完成不确认"),
    ], [1200, 3200, 4960])
    add_heading(doc, "6.3 自动收起", 2)
    add_para(doc, "录制开始 2 秒后，若鼠标离开状态条，则收起为“红点 + 时间 + 完成”的紧凑态；鼠标靠近或键盘聚焦时展开。用户可在设置中关闭自动收起。条的位置由用户拖动后按显示器记忆。")
    add_heading(doc, "6.4 取消确认", 2)
    add_callout(doc, "确认文案", "取消这次录制？尚未生成的视频将被移入临时恢复区，并在 24 小时后自动清理。按钮：继续录制 / 取消并保留临时文件。")

    add_heading(doc, "7. 结果页：双版本交付", 1)
    add_heading(doc, "7.1 页面结构", 2)
    add_table(doc, ["区域", "内容"], [
        ("顶部", "可编辑名称、录制时间、打开文件夹、更多"),
        ("播放器", "默认轻量版；角标显示“轻量预览”；可切换高清"),
        ("版本卡片 A", "轻量预览：状态、格式、尺寸、体积、复制/拖出；优先完成"),
        ("版本卡片 B", "高清母版：状态、格式、尺寸、体积、打开/导出；后台进度"),
        ("底部动作", "再次录制、完成；若设置为完成后自动复制则显示轻提示"),
    ], [1900, 7460])
    add_heading(doc, "7.2 处理状态", 2)
    add_table(doc, ["状态", "版本卡片表现", "用户可做什么"], [
        ("等待", "灰色骨架 + “等待编码资源”", "可继续使用已完成版本"),
        ("处理中", "确定/不确定进度、预计时间、取消按钮", "可关闭窗口，后台继续"),
        ("完成", "显示播放/复制/打开与完整元数据", "可重新导出"),
        ("失败", "红色说明 + 简化原因 + 重试", "查看日志、改用兼容格式"),
        ("已取消", "保留重新生成入口", "从母版重新生成"),
    ], [1500, 4300, 3560])
    add_heading(doc, "7.3 默认主动作", 2)
    add_bullets(doc, [
        "轻量版完成且高清处理中：主动作是“复制轻量版”。",
        "两者都完成：主动作保持“复制轻量版”，次动作“打开高清版”。",
        "轻量版失败但高清完成：主动作切换为“打开高清版”，并提示可重试轻量版。",
        "关闭结果页不会中断后台处理；菜单栏/托盘显示进度与完成通知。",
    ])

    add_heading(doc, "8. 设置窗口", 1)
    add_heading(doc, "8.1 导航", 2)
    add_para(doc, "左侧分组：通用、录制、高清输出、轻量输出、快捷键、存储、隐私与更新。窗口宽度建议 760–860px，内容区最大 640px，避免表单跨越过宽。")
    add_heading(doc, "8.2 高清输出", 2)
    add_table(doc, ["字段", "默认值", "说明/约束"], [
        ("格式", "MP4", "MP4 / MOV / WebM；根据容器过滤编码器"),
        ("编码", "H.264 硬件加速", "H.264 / HEVC / VP9 / AV1（按能力显示）"),
        ("分辨率", "原始", "原始 / 1080p / 720p / 自定义；不默认放大"),
        ("帧率", "30 fps", "24 / 30 / 60；不得超过采集帧率"),
        ("质量", "高质量", "节省空间 / 平衡 / 高质量 / 自定义"),
        ("音频", "AAC 128 kbps", "可关闭；格式随容器约束"),
    ], [1700, 2200, 5460])
    add_heading(doc, "8.3 轻量输出", 2)
    add_table(doc, ["字段", "默认值", "说明/约束"], [
        ("优化目标", "最佳压缩", "最佳压缩 / 最佳兼容 / 自定义"),
        ("格式", "WebM", "最佳兼容时自动切换 MP4"),
        ("尺寸", "长边 960px", "640 / 960 / 1280 / 自定义"),
        ("帧率", "15 fps", "15 / 24 / 30"),
        ("体积", "小", "更小 / 小 / 平衡；显示预估区间"),
        ("音频", "Opus 64 kbps", "可设为静音预览"),
    ], [1700, 2200, 5460])
    add_callout(doc, "设置交互", "切换“最佳压缩 / 最佳兼容”时，用一句话解释结果，并在下方实时展示示例：预计 1 分钟视频约 6–10MB。高级编码参数默认折叠。", "green")

    page_break(doc)
    add_heading(doc, "9. 快捷键", 1)
    add_table(doc, ["动作", "macOS", "Windows", "上下文"], [
        ("开始区域录制", "⌥⌘R", "Alt+Shift+R", "全局"),
        ("开始/确认", "Return", "Enter", "选区就绪"),
        ("取消/返回", "Esc", "Esc", "选择/倒计时"),
        ("暂停/继续", "⌥⌘P", "Alt+Shift+P", "全局，录制中"),
        ("完成并保存", "⌥⌘S", "Alt+Shift+S", "全局，录制中"),
        ("麦克风静音", "⌥⌘M", "Alt+Shift+M", "全局，录制中"),
        ("锁定比例", "Shift（按住）", "Shift（按住）", "拖拽选区"),
        ("精细移动", "方向键 / Shift+方向键", "方向键 / Shift+方向键", "选区就绪"),
    ], [2300, 2100, 2300, 2660])
    add_para(doc, "全局快捷键必须允许修改并检测冲突；冲突时提示冲突应用不可完全可靠识别，因此提供“重新录入”与“恢复默认”。")

    add_heading(doc, "10. 键盘与可访问性", 1)
    add_bullets(doc, [
        "选择模式下，Tab 在模式、比例、尺寸、输入、开始录制、更多之间循环；焦点环 2px，和选区边框视觉区分。",
        "状态变化通过屏幕阅读器友好 live region 播报：“录制已开始”“已暂停”“轻量版已完成”。",
        "红/琥珀/绿状态必须同时有图标与文字；麦克风电平提供“无输入/正常/过载”文本替代。",
        "支持 200% UI 缩放；状态条在空间不足时收敛，不裁切主要动作；最小点击目标 36×36。",
        "减少动态效果模式下移除缩放和位移动画，仅保留 80ms 透明度变化。",
        "高对比度模式使用系统颜色；遮罩边框增加双线轮廓，保证复杂背景可见。",
    ])

    add_heading(doc, "11. 反馈、提示与错误", 1)
    add_table(doc, ["场景", "呈现", "推荐文案"], [
        ("权限缺失", "悬浮菜单内联提示 + 修复按钮", "需要屏幕录制权限才能开始。打开系统设置"),
        ("麦克风无输入", "图标黄点，非阻塞", "没有检测到声音。检查麦克风或继续静音录制"),
        ("空间不足", "录前阻塞对话框", "预计需要 2.4GB，当前可用 1.1GB"),
        ("显示器断开", "立即暂停，状态条警告", "录制区域所在显示器已断开。选择新区域后继续"),
        ("轻量版完成", "系统通知 + 结果页卡片", "轻量预览已就绪，可复制分享"),
        ("高清失败", "结果页卡片错误，不影响低清", "高清生成失败。可重试或改用兼容格式"),
        ("恢复文件", "启动后顶部恢复卡片", "发现一段未完成录制，可尝试恢复"),
    ], [1600, 3000, 4760])
    add_heading(doc, "11.1 Toast 规则", 2)
    add_bullets(doc, [
        "用于已完成且无需决策的轻反馈，如“轻量版已复制”“保存位置已更新”。",
        "显示 3 秒，可聚焦，悬停暂停消失；不堆叠超过 3 条。",
        "错误若需要行动，不使用短暂 Toast，改用内联错误或对话框。",
    ])

    add_heading(doc, "12. 动效与视觉状态", 1)
    add_table(doc, ["对象", "参数", "意图"], [
        ("悬浮菜单出现", "opacity 0→1 + y 4→0，140ms ease-out", "确认选区已就绪"),
        ("比例切换", "边框位置/尺寸 160ms ease-out", "保持空间连续性"),
        ("录制开始", "主按钮压下 80ms；状态点淡入", "明确因果，不闪屏"),
        ("暂停", "红→琥珀 120ms；文字切换", "让状态改变被感知"),
        ("版本完成", "进度条完成后淡出；勾选淡入", "不使用庆祝动画打断流程"),
    ], [1800, 4100, 3460])

    add_heading(doc, "13. 响应式与多显示器", 1)
    add_bullets(doc, [
        "每个显示器单独创建覆盖层，按实际缩放因子转换逻辑像素与物理像素。",
        "选区不可跨不同缩放因子的显示器；若产品后续支持跨屏录制，需单独定义拼接与帧率策略。",
        "悬浮菜单始终约束在当前显示器工作区，不遮挡系统任务栏/Dock 的保留区域。",
        "选区宽度 < 520px 时进入紧凑菜单；< 360px 时只保留比例、输入摘要和开始录制。",
        "主窗口最小 720×520；最近列表在窄宽度改为单列卡片。",
    ])

    add_heading(doc, "14. 交互验收用例", 1)
    add_table(doc, ["用例", "通过条件"], [
        ("快速首录", "首次用户从主窗口到倒计时开始 ≤ 30 秒，且无需进入设置"),
        ("比例切换", "任意选区切换 16:9 后仍在屏幕内，中心偏移 ≤ 1px（无边界限制时）"),
        ("菜单自适应", "选区贴近屏幕四边/四角时，菜单无裁切且主按钮始终可用"),
        ("双版本异步", "轻量版先完成即可播放/复制；关闭结果页不终止高清任务"),
        ("单版本失败", "另一个版本保持可用，失败卡片能重试并显示可理解原因"),
        ("键盘路径", "不使用鼠标可完成选区调整、开始、暂停、完成、复制轻量版"),
        ("辅助技术", "所有状态与控件有可访问名称，状态变化可被读取"),
        ("高 DPI", "Windows 125/150/200% 与 macOS Retina 下边框、控制点和视频尺寸准确"),
    ], [2600, 6760])

    add_heading(doc, "15. 设计稿评审清单", 1)
    add_bullets(doc, [
        "第一眼能否看出选中的录制区域，以及下一步从哪里开始？",
        "悬浮条是否在复杂屏幕内容上仍保持清晰，但不显得厚重？",
        "比例、尺寸与输入状态是否在一行内建立正确优先级？",
        "录制态与暂停态是否只看轮廓就能区分？",
        "结果页是否把两个版本理解成同一次录制的不同用途，而不是两个重复文件？",
        "高级格式设置是否保持专业能力，同时不迫使普通用户理解编码器？",
        "macOS 与 Windows 是否同源而不生硬复制系统外观？",
    ])
    path = OUT / "FrameFlow_Interaction_Spec_v1.0.docx"
    doc.save(path)
    return path


if __name__ == "__main__":
    p1 = build_prd()
    p2 = build_interaction()
    print(p1)
    print(p2)
