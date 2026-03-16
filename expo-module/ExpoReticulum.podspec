require 'json'
package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name           = 'ExpoReticulum'
  s.version        = package['version']
  s.summary        = package['description']
  s.license        = package['license']
  s.homepage       = 'https://github.com/anon0mesh/anon0mesh'
  s.authors        = 'anon0mesh'
  s.platform       = :ios, '14.0'
  s.swift_version  = '5.9'

  s.source       = { git: '' }
  s.source_files = 'ios/**/*.{swift,h,m}'

  # The Rust static library built by build_rust_ios.sh
  s.vendored_frameworks = 'ios/Frameworks/ReticulumMobile.xcframework'

  s.dependency 'ExpoModulesCore'

  # Run the Rust build as a prepare command so pod install triggers it.
  # Xcode build phase script handles incremental rebuilds.
  s.prepare_command = 'bash ios/build_rust_ios.sh'
end
