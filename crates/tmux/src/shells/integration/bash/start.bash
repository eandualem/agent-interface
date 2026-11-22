# Agent Interface starts bash as `bash --rcfile <this file> -i`, so this file
# runs in place of ~/.bashrc. It reads the start-up files a login shell reads,
# as `bash -l` would, then loads the guard. (`--rcfile` because the bash 3.2
# that macOS ships ignores ENV in POSIX mode.)
__agent_interface_dir=${BASH_SOURCE[0]%/*}/..
if [ -r /etc/profile ]; then
  builtin source /etc/profile
fi
for __agent_interface_file in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
  if [ -r "$__agent_interface_file" ]; then
    builtin source "$__agent_interface_file"
    break
  fi
done
builtin source "$__agent_interface_dir/guard.sh"
builtin unset __agent_interface_dir __agent_interface_file
