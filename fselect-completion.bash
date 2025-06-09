#!/bin/bash

_fselect_complete() {
    _init_completion || return

    for ((i=COMP_CWORD-1; i>=0; i--)); do
        local word="${COMP_WORDS[i]}"
        if [[ "${word,,}" == "from" ]]; then
            _filedir -d
        elif [[ "${word,,}" == "into" ]]; then
            local output_formats=$(fselect --output-formats)
            COMPREPLY=($(compgen -W "$output_formats" -- "${COMP_WORDS[COMP_CWORD]}"))
        else
            local fields=$(fselect --fields)
            local functions=$(fselect --functions)
            COMPREPLY=($(compgen -W "$fields $functions" -- "${COMP_WORDS[COMP_CWORD]}"))
        fi
    done
}

complete -F _fselect_complete fselect
