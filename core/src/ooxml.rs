//! Minimal OOXML writers (zero dependencies): stored (uncompressed) ZIP +
//! hand-rolled XML. Output opens in real Office/LibreOffice. Scope: single
//! sheet workbooks and simple paragraph/table documents — transfer targets,
//! not full fidelity. PPTX deferred (DrawingML surface too large for POC).

/// CRC32 (ISO 3309, table-free bitwise implementation).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\r' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn le16(v: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le32(v: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Stored-entry ZIP writer. Returns the archive bytes.
pub fn zip_store(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let len = data.len() as u32;
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]); // local header
        le16(20, &mut out);
        le16(0, &mut out); // flags
        le16(0, &mut out); // method: stored
        le16(0, &mut out);
        le16(0, &mut out); // time/date
        le32(crc, &mut out);
        le32(len, &mut out);
        le32(len, &mut out);
        le16(name.len() as u16, &mut out);
        le16(0, &mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]); // central header
        le16(20, &mut central);
        le16(20, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le32(crc, &mut central);
        le32(len, &mut central);
        le32(len, &mut central);
        le16(name.len() as u16, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le16(0, &mut central);
        le32(0, &mut central);
        le32(offset, &mut central);
        central.extend_from_slice(name.as_bytes());
    }
    let central_start = out.len() as u32;
    out.extend_from_slice(&central);
    let central_len = (out.len() as u32) - central_start;
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]); // end record
    le16(0, &mut out);
    le16(0, &mut out);
    le16(files.len() as u16, &mut out);
    le16(files.len() as u16, &mut out);
    le32(central_len, &mut out);
    le32(central_start, &mut out);
    le16(0, &mut out);
    out
}

fn col_name(mut i: usize) -> String {
    let mut s = String::new();
    i += 1;
    while i > 0 {
        let m = (i - 1) % 26;
        s.insert(0, (b'A' + m as u8) as char);
        i = (i - 1) / 26;
    }
    s
}

/// Minimal single-sheet .xlsx bytes from a string grid.
pub fn write_xlsx(sheet: &str, grid: &[Vec<String>]) -> Vec<u8> {
    let mut rows = String::new();
    for (i, row) in grid.iter().enumerate() {
        rows.push_str(&format!("<row r=\"{}\">", i + 1));
        for (j, v) in row.iter().enumerate() {
            let cell = format!("{}{}", col_name(j), i + 1);
            // Numbers stay numeric (formulas keep working); text goes inline.
            if v.trim().parse::<f64>().is_ok() && !v.trim().is_empty() {
                rows.push_str(&format!("<c r=\"{cell}\"><v>{}</v></c>", escape_xml(v.trim())));
            } else {
                rows.push_str(&format!(
                    "<c r=\"{cell}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    escape_xml(v)
                ));
            }
        }
        rows.push_str("</row>");
    }
    let files = vec![
        ("[Content_Types].xml".into(), br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".into(), br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_vec()),
        ("xl/workbook.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets></workbook>"#, escape_xml(sheet)).into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_vec()),
        ("xl/worksheets/sheet1.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#).into_bytes()),
    ];
    zip_store(&files)
}

/// Minimal .docx bytes from paragraphs + tables (each table = grid of strings).
pub fn write_docx(paras: &[String], tables: &[Vec<Vec<String>>]) -> Vec<u8> {
    let mut body = String::new();
    for p in paras {
        if p.starts_with("[table") || p.starts_with("[imported table") {
            continue; // marker paras are rendering hints, not content
        }
        body.push_str(&format!("<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", escape_xml(p)));
    }
    for grid in tables {
        body.push_str("<w:tbl>");
        for row in grid {
            body.push_str("<w:tr>");
            for cell in row {
                body.push_str(&format!("<w:tc><w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p></w:tc>", escape_xml(cell)));
            }
            body.push_str("</w:tr>");
        }
        body.push_str("</w:tbl>");
    }
    let files = vec![
        ("[Content_Types].xml".into(), br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_vec()),
        ("_rels/.rels".into(), br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec()),
        ("word/document.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#).into_bytes()),
    ];
    zip_store(&files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn zip_starts_with_pk_and_roundtrips_names() {
        let z = zip_store(&[("a.txt".into(), b"hi".to_vec())]);
        assert_eq!(&z[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("a.txt"));
    }

    #[test]
    fn xlsx_embeds_values() {
        let z = write_xlsx("Sheet1", &[vec!["item".into(), "plan".into()], vec!["ads".into(), "100".into()]]);
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("Sheet1") && s.contains("ads") && s.contains("100"));
    }

    #[test]
    fn docx_embeds_paras_and_tables() {
        let z = write_docx(&["Q3 Report".into(), "[imported table 3 rows]".into()], &[vec![vec!["a".into(), "b".into()]]]);
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("Q3 Report") && s.contains("<w:tbl>") && !s.contains("[imported table"));
    }

    #[test]
    fn escape_neutralizes_markup() {
        assert_eq!(escape_xml("<b>&"), "&lt;b&gt;&amp;");
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn numbers_stay_numeric_text_inline() {
        let z = write_xlsx("N", &[vec!["42".into(), "3.5".into(), " 7 ".into(), "4a".into(), "".into()]]);
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("<c r=\"A1\"><v>42</v></c>"));
        assert!(s.contains("<c r=\"B1\"><v>3.5</v></c>"));
        assert!(s.contains("t=\"inlineStr\""));
        assert!(!s.contains("<c r=\"A2\"")); // single row only
    }

    #[test]
    fn col_names_roll_past_z() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
    }

    #[test]
    fn crc32_empty_and_single() {
        assert_eq!(crc32(b""), 0);
        assert_ne!(crc32(b"a"), crc32(b"b"));
    }

    #[test]
    fn docx_escapes_and_empty_tables() {
        let z = write_docx(&["a<b".into()], &[]);
        let s = String::from_utf8_lossy(&z);
        assert!(s.contains("a&lt;b") && !s.contains("<w:tbl>"));
        let z2 = write_docx(&[], &[]);
        assert_eq!(&z2[..4], &[0x50, 0x4b, 0x03, 0x04]);
    }
}
