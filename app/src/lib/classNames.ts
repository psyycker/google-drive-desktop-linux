/** Joins the truthy class names: `cx('btn', primary && 'btn-primary')`. */
export const cx = (...names: (string | false | null | undefined)[]) => names.filter(Boolean).join(' ');
