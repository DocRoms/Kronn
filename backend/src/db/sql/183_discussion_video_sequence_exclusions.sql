-- Clips a person set aside from the film (Assets > Editor): kept by the
-- discussion but not played. A clip in neither list joins the film's end.
ALTER TABLE discussion_video_sequences
    ADD COLUMN excluded_file_ids_json TEXT NOT NULL DEFAULT '[]';
