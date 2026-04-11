export const debug = {
    enabled: false,
    enable(val) {
        this.enabled = val ? true : false;
    },
    log(...args) {
        if (this.enabled) {
            console.log(...args);
        }
    }
};
