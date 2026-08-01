-- Identificação por PASTA, não por arquivo.
--
-- POR QUE ESTA MIGRAÇÃO EXISTE
-- Medido num acervo real de 17.503 arquivos: o backlog de identificação era de
-- 7.568 arquivos, mas apenas **578 diretórios**. E o diretório é uma chave
-- confiável — entre os 487 diretórios que já tinham episódios casados, 474
-- (97,3%) apontavam para exatamente uma série. O nome da série está na pasta,
-- limpo, mesmo quando o nome do arquivo é ilegível.
--
-- A consequência de projeto é que a unidade de decisão humana passa a ser a
-- pasta. Uma escolha em `/media2/TV Show/Naruto Shippuden` resolve 499 arquivos.

-- O diretório do arquivo, materializado.
--
-- GENERATED e não uma view: as consultas de escopo agrupam por diretório o
-- tempo todo, e `regexp_replace(path, ...)` num WHERE não usa índice — seria
-- sequential scan sobre 17 mil linhas a cada abertura da fila.
ALTER TABLE media_file ADD COLUMN dir_path text
    GENERATED ALWAYS AS (regexp_replace(path, '/[^/]+$', '')) STORED;

CREATE INDEX media_file_dir_idx ON media_file (dir_path);

-- A decisão humana sobre uma pasta, PERSISTIDA.
--
-- É isto que impede o backlog de voltar: o scan do mês que vem acha 20
-- episódios novos na mesma pasta e eles casam sozinhos, pelo mesmo escopo e com
-- as mesmas razões gravadas. Sem persistir, cada varredura recria a fila.
CREATE TABLE identification_scope (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id    uuid NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    dir_path      text NOT NULL,
    -- true: vale para a subárvore inteira — série com pastas de temporada
    -- dentro. false: só os arquivos diretamente nesta pasta.
    recursive     boolean NOT NULL DEFAULT false,

    provider      text NOT NULL CHECK (provider IN ('tmdb', 'anilist')),
    provider_id   text NOT NULL,
    provider_kind text NOT NULL CHECK (provider_kind IN ('movie', 'tv', 'anime')),

    -- Fixa a temporada de toda a pasta quando o nome do arquivo não a diz.
    -- NULL = cada arquivo resolve a sua.
    season_number int,

    -- Como ler o número do episódio:
    --   seasonal: SxxExx manda
    --   absolute: numeração corrida (fansub), mapeada pro par temporada/episódio
    --   none:     a pasta não é serial (coletânea de filmes)
    numbering     text NOT NULL DEFAULT 'seasonal'
                  CHECK (numbering IN ('seasonal', 'absolute', 'none')),
    -- Ajuste fino do mapeamento absoluto. A numeração de fansub diverge das
    -- fronteiras de temporada do provider com frequência; sem isto a única saída
    -- seria corrigir arquivo por arquivo.
    absolute_offset int NOT NULL DEFAULT 0,

    decided_by    uuid REFERENCES app_user(id) ON DELETE SET NULL,
    decided_at    timestamptz NOT NULL DEFAULT now(),
    note          text,

    -- Uma decisão por pasta. Redecidir é UPDATE, não uma segunda linha —
    -- senão não haveria como saber qual vale.
    UNIQUE (library_id, dir_path)
);

CREATE INDEX identification_scope_dir_idx ON identification_scope (dir_path);

-- Por que a obra ficou como ficou, quando NÃO houve candidato.
--
-- `match_candidate.reasons` cobre o caminho normal, mas uma obra identificada
-- por propagação de escopo não tem candidato próprio — e ficaria sem o "porquê"
-- que o §8b exige de toda decisão. Esta coluna é aquele contrato estendido às
-- decisões que não passam por candidato.
ALTER TABLE work ADD COLUMN match_reasons jsonb NOT NULL DEFAULT '[]'::jsonb;

-- A fila é lida ordenada por confiança e filtrada por estado. Sem isto, cada
-- página faz sort sobre a tabela inteira.
CREATE INDEX work_review_idx ON work (match_state, match_confidence DESC)
    WHERE match_state IN ('needs_review', 'unmatched');
