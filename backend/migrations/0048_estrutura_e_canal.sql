-- A estrutura deixa de depender da identificação — **R75**.
--
-- ## O que estava conflado
--
-- Uma `collection(series)` só nascia quando o provider confirmava a série. O
-- efeito, medido em 20/08/2026: **507 pastas do acervo são claramente série**
-- (14.141 arquivos com numeração), e existiam **133 séries**. As outras 196
-- pastas não tinham série nenhuma no banco, e seus episódios apareciam como
-- cartões soltos na grade.
--
-- Mas "isto é uma série" e "esta série é `tmdb/16183`" são perguntas
-- diferentes, e só a segunda precisa de rede. É a mesma separação que o R64
-- fez com o formato — *dá pra saber que algo é um filme sem saber que filme é*.
--
-- ## As três palavras novas
--
-- `origin = 'estrutura'` — a coleção veio do **disco**, não de uma pessoa nem
-- de um provider. A distinção importa no `reset`, que tem de apagar o que o
-- provider trouxe e preservar o que alguém montou; agora há um terceiro caso,
-- e ele é recriável de graça a qualquer momento.
--
-- `collection.kind = 'channel'` e `work.kind = 'video'` — o YouTube. São 2.511
-- arquivos em 12 canais, hoje gravados como `other`, que quer dizer "o scanner
-- não sabe o que é isto". Numa biblioteca declarada `provider_hint = 'none'` o
-- scanner **sabe**: é vídeo de canal, e nunca vai ter ficha no TMDB. Chamar
-- isso de "não sei" é a única mentira que o modelo ainda contava sobre eles.

ALTER TABLE collection DROP CONSTRAINT collection_origin_check;
ALTER TABLE collection ADD CONSTRAINT collection_origin_check
    CHECK (origin IN ('manual', 'provider', 'estrutura'));

ALTER TABLE collection DROP CONSTRAINT collection_kind_check;
ALTER TABLE collection ADD CONSTRAINT collection_kind_check
    CHECK (kind IN ('series', 'season', 'franchise', 'playlist', 'watch_order',
                    'custom', 'channel'));

ALTER TABLE work DROP CONSTRAINT work_kind_check;
ALTER TABLE work ADD CONSTRAINT work_kind_check
    CHECK (kind IN ('movie', 'episode', 'short', 'standup', 'concert',
                    'documentary', 'music_video', 'other', 'unknown', 'video'));

-- A biblioteca do YouTube passa a declarar o que ela é. `default_kind` é o que
-- o scanner grava em arquivo novo; sem isto, o próximo vídeo baixado voltaria
-- a nascer `other`.
UPDATE library SET default_kind = 'video'
 WHERE provider_hint = 'none' AND default_kind = 'other';

-- E o que já está lá acompanha. Só `other` — `music_video` da biblioteca de
-- clipes é outra coisa e continua sendo.
UPDATE work w SET kind = 'video', updated_at = now()
 WHERE w.kind = 'other'
   AND EXISTS (
       SELECT 1 FROM media_file mf JOIN library l ON l.id = mf.library_id
        WHERE mf.work_id = w.id AND l.default_kind = 'video');

-- Onde a arte do canal vai morar. `external_ids` já existe em `collection` e
-- guarda `{"youtube": "UC…"}` quando alguém confirmar o canal — ver R76.
CREATE INDEX collection_estrutura_idx
    ON collection (provider_key) WHERE origin = 'estrutura';
