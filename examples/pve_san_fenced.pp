# @summary Manages the installation, configuration, and service of the Proxmox VE SAN fencing daemon.
#
# @param poll_interval
#   Interval in seconds between multipathd checks.
# @param max_failures
#   Number of consecutive failures before fencing is triggered.
# @param discovery_interval
#   Interval in seconds between VM and storage discovery scans.
# @param socket
#   Multipathd socket path (e.g. '@/org/kernel/linux/storage/multipathd').
# @param sysrq_char
#   Comma-separated list of SysRq characters to send sequentially on fence (e.g. 's,b').
# @param test_mode
#   Set to true to run in test/dry-run mode (does not trigger reboot/panic).
# @param debug
#   Set to true to enable verbose debug logging of discovered VMs/storages.
# @param package_name
#   The name of the package to install.
# @param service_name
#   The name of the systemd service to manage.
class pve_san_fenced (
  Integer $poll_interval       = 5,
  Integer $max_failures        = 6,
  Integer $discovery_interval  = 60,
  String  $socket              = '@/org/kernel/linux/storage/multipathd',
  String  $sysrq_char          = 's,b',
  Boolean $test_mode           = false,
  Boolean $debug               = false,
  String  $package_name        = 'pve-san-fenced',
  String  $service_name        = 'pve-san-fenced',
) {
  package { $package_name:
    ensure => installed,
  }

  $config_content = @("CONFIG")
    # Configuration for pve-san-fenced daemon

    # Poll interval in seconds
    PVE_SAN_POLL_INTERVAL=${poll_interval}

    # Maximum consecutive failures before fencing
    PVE_SAN_MAX_FAILURES=${max_failures}

    # Discovery interval in seconds
    PVE_SAN_DISCOVERY_INTERVAL=${discovery_interval}

    # Multipathd socket path
    PVE_SAN_SOCKET=${socket}

    # SysRq character to trigger fencing (default is 's,b' for sync and reboot)
    PVE_SAN_SYSRQ_CHAR=${sysrq_char}

    # Set to true to run in test/dry-run mode (does not trigger SysRq kernel panic)
    PVE_SAN_TEST_MODE=${test_mode}

    # Set to true to enable verbose debug logging of discovered VMs, storages, and multipaths on each discovery run
    PVE_SAN_DEBUG=${debug}
    |-CONFIG

  file { '/etc/default/pve-san-fenced':
    ensure  => file,
    owner   => 'root',
    group   => 'root',
    mode    => '0644',
    content => $config_content,
    require => Package[$package_name],
    notify  => Service[$service_name],
  }

  service { $service_name:
    ensure    => running,
    enable    => true,
    subscribe => File['/etc/default/pve-san-fenced'],
  }
}
