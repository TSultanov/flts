export type PlayerState = 'playing' | 'paused' | 'stopped' | 'notrunning';

export type NowPlaying = {
    state: PlayerState;
    trackId?: string;
    name?: string;
    artist?: string;
    album?: string;
    positionMs?: number;
    durationMs?: number;
};

export type LyricsLine = {
    time_ms: number | null;
    text: string;
};

export type Lyrics = {
    track_id: string;
    lines: LyricsLine[];
    synced: boolean;
};

export type Gloss = {
    fragment: string;
    gloss: string;
    note: string;
};

export type LyricsLineTranslation = {
    translation: string;
    glosses: Gloss[];
};

export type LyricsTranslation = {
    track_id: string;
    target_lang: string;
    model: number;
    lines: LyricsLineTranslation[];
};

export type LyricsTranslationProgress = {
    requestId: number;
    bytes: number;
};

export type LyricsTranslationDone = {
    requestId: number;
    translation: LyricsTranslation;
};

export type LyricsTranslationError = {
    requestId: number;
    error: string;
};
