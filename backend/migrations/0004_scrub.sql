-- M3 — preview de seek (scrub).
--
-- Uma folha de sprites por arquivo: N quadros amostrados em intervalo fixo,
-- ladrilhados numa imagem só. O player calcula qual célula mostrar a partir do
-- tempo sob o cursor — uma requisição, zero latência ao arrastar.
--
-- Por que folha única e não N arquivos: arrastar a timeline dispararia dezenas
-- de requests por segundo. Com a folha, o browser baixa uma vez e o resto é
-- `background-position`.

CREATE TABLE scrub_sprite (
    media_file_id    uuid PRIMARY KEY REFERENCES media_file(id) ON DELETE CASCADE,
    path             text NOT NULL,
    interval_seconds real NOT NULL,
    columns          int NOT NULL,
    rows             int NOT NULL,
    thumb_width      int NOT NULL,
    thumb_height     int NOT NULL,
    frame_count      int NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now()
);
