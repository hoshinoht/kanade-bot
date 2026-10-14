/** `HH:MM`, 24-hour, as the server stores `v5.header_generation_time`. */
export const validClock = (value: string): boolean => /^([01]\d|2[0-3]):[0-5]\d$/.test(value.trim());
