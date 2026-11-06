#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ ! -s "$project_dir/.vm/uuid" ]]; then
    printf 'The dedicated iCloud VM was not found.\n' >&2
    exit 1
fi
IFS= read -r vm_uuid < "$project_dir/.vm/uuid"
if [[ ! "$vm_uuid" =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]]; then
    printf 'The VM identifier is invalid.\n' >&2
    exit 1
fi

vm_info="$(VBoxManage showvminfo "$vm_uuid" --machinereadable)"
if [[ "$vm_info" == *'VMState="running"'* ]]; then
    open -a VirtualBox
    printf 'Ubuntu-iCloud is running. Select it in VirtualBox and choose Show.\n'
else
    VBoxManage startvm "$vm_uuid" --type gui
fi
